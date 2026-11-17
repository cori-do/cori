// SPDX-License-Identifier: Apache-2.0
// Package cori helps a Temporal worker written with the official Go SDK serve Cori native activities:
// the envelope Cori sends, the run context, principal verification against the org's OIDC issuer, and
// the error type Cori matches against `retry.non_retryable` in activities.yaml.
package cori

import (
	"context"
	"crypto"
	"crypto/aes"
	"crypto/cipher"
	"crypto/rsa"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"math/big"
	"net/http"
	"os"
	"strings"
	"sync"
	"time"

	"go.temporal.io/sdk/temporal"
)

// Principal is the run's user as Cori resolved it (roles from the IdP or the app's role map).
type Principal struct {
	Sub   string   `json:"sub"`
	Roles []string `json:"roles"`
	Email string   `json:"email,omitempty"`
}

// Context is the `cori` half of the envelope: who, which run, which step, and an idempotency key
// that is stable across retries of the same step in the same run. PrincipalToken is sealed for this
// connector's worker (Temporal writes the envelope into the run's history): read it with Token.
type Context struct {
	RunID          string    `json:"run_id"`
	Workflow       string    `json:"workflow"`
	Version        string    `json:"version"`
	Step           string    `json:"step"`
	Attempt        int       `json:"attempt"`
	User           Principal `json:"user"`
	PrincipalToken string    `json:"principal_token,omitempty"`
	IdempotencyKey string    `json:"idempotency_key"`
}

// SealedPrefix starts a token Cori sealed for a connector's worker.
const SealedPrefix = "cori-sealed.v1."

// Token is the run's token for this connector, opened with the connector's key (CORI_CONNECTOR_KEY, hex, from
// `cori connectors key <name>`); "" when the envelope has none (a local run, a connector without an audience).
func (c Context) Token() (string, error) {
	if !strings.HasPrefix(c.PrincipalToken, SealedPrefix) {
		return c.PrincipalToken, nil
	}
	key, err := hex.DecodeString(strings.TrimSpace(os.Getenv("CORI_CONNECTOR_KEY")))
	if err != nil || len(key) != 32 {
		return "", errors.New("CORI_CONNECTOR_KEY is not this connector's key (64 hex digits: `cori connectors key <name>`)")
	}
	return OpenToken(c.PrincipalToken, c.RunID, key)
}

// OpenToken opens a token Cori sealed for a connector's worker: base64url of nonce (12 bytes), ciphertext and tag,
// AES-256-GCM under the connector's key, the run id as associated data (a token opens for its own run only).
func OpenToken(sealed, runID string, key []byte) (string, error) {
	b, err := base64.RawURLEncoding.DecodeString(strings.TrimPrefix(sealed, SealedPrefix))
	if !strings.HasPrefix(sealed, SealedPrefix) || err != nil || len(b) < 12+16 {
		return "", errors.New("not a sealed token")
	}
	block, err := aes.NewCipher(key)
	if err != nil {
		return "", fmt.Errorf("connector key: %w", err)
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return "", err
	}
	plain, err := gcm.Open(nil, b[:12], b[12:], []byte(runID))
	if err != nil {
		return "", errors.New("the token does not open with this key for this run")
	}
	return string(plain), nil
}

// Envelope is what every Cori-dispatched activity receives: the typed input and the run context.
type Envelope[T any] struct {
	Input T       `json:"input"`
	Cori  Context `json:"cori"`
}

// NonRetryable returns an application error whose type Cori matches against `retry.non_retryable`
// (e.g. "Forbidden", "InvoiceNotFound"); Temporal will not retry it.
func NonRetryable(errType string, err error) error {
	return temporal.NewNonRetryableApplicationError(err.Error(), errType, err)
}

// HasRole reports whether the run's user carries any of the given roles.
func (c Context) HasRole(roles ...string) bool {
	for _, want := range roles {
		for _, have := range c.User.Roles {
			if have == want {
				return true
			}
		}
	}
	return false
}

// Verifier checks the exchanged principal token (RS256 JWT) against the issuer's JWKS: signature,
// expiry, issuer and audience. Keys are cached; a key id that is not cached triggers one refresh.
type Verifier struct {
	Issuer   string
	Audience string
	Client   *http.Client
	mu       sync.Mutex
	keys     map[string]*rsa.PublicKey
	fetched  time.Time
}

// NewVerifier builds a verifier for `issuer` (its `/.well-known/openid-configuration` gives the JWKS URI).
func NewVerifier(issuer, audience string) *Verifier {
	return &Verifier{Issuer: strings.TrimRight(issuer, "/"), Audience: audience, Client: &http.Client{Timeout: 10 * time.Second}}
}

// Claims is the verified payload of a principal token.
type Claims struct {
	Sub   string   `json:"sub"`
	Iss   string   `json:"iss"`
	Aud   any      `json:"aud"`
	Exp   int64    `json:"exp"`
	Roles []string `json:"roles"`
	Email string   `json:"email"`
}

// Verify parses and verifies the token; it returns the claims or an error describing what failed.
func (v *Verifier) Verify(ctx context.Context, token string) (*Claims, error) {
	parts := strings.Split(token, ".")
	if len(parts) != 3 {
		return nil, errors.New("not a JWT")
	}
	var header struct {
		Alg string `json:"alg"`
		Kid string `json:"kid"`
	}
	hb, err := base64.RawURLEncoding.DecodeString(parts[0])
	if err != nil || json.Unmarshal(hb, &header) != nil {
		return nil, errors.New("bad JWT header")
	}
	if header.Alg != "RS256" {
		return nil, fmt.Errorf("unsupported alg %s", header.Alg)
	}
	key, err := v.key(ctx, header.Kid)
	if err != nil {
		return nil, err
	}
	sig, err := base64.RawURLEncoding.DecodeString(parts[2])
	if err != nil {
		return nil, errors.New("bad signature encoding")
	}
	sum := sha256.Sum256([]byte(parts[0] + "." + parts[1]))
	if err := rsa.VerifyPKCS1v15(key, crypto.SHA256, sum[:], sig); err != nil {
		return nil, errors.New("invalid signature")
	}
	pb, err := base64.RawURLEncoding.DecodeString(parts[1])
	if err != nil {
		return nil, errors.New("bad payload encoding")
	}
	var c Claims
	if err := json.Unmarshal(pb, &c); err != nil {
		return nil, errors.New("bad payload")
	}
	if c.Exp > 0 && time.Now().Unix() > c.Exp {
		return nil, errors.New("token expired")
	}
	if v.Issuer != "" && c.Iss != v.Issuer {
		return nil, fmt.Errorf("issuer %q is not %q", c.Iss, v.Issuer)
	}
	if v.Audience != "" && !hasAud(c.Aud, v.Audience) {
		return nil, fmt.Errorf("audience does not include %q", v.Audience)
	}
	return &c, nil
}

func hasAud(aud any, want string) bool {
	switch a := aud.(type) {
	case string:
		return a == want
	case []any:
		for _, x := range a {
			if s, ok := x.(string); ok && s == want {
				return true
			}
		}
	}
	return false
}

func (v *Verifier) key(ctx context.Context, kid string) (*rsa.PublicKey, error) {
	v.mu.Lock()
	defer v.mu.Unlock()
	if k, ok := v.keys[kid]; ok {
		return k, nil
	}
	if time.Since(v.fetched) < 30*time.Second && v.keys != nil {
		return nil, fmt.Errorf("unknown key id %q", kid)
	}
	var disc struct {
		JwksURI string `json:"jwks_uri"`
	}
	if err := v.getJSON(ctx, v.Issuer+"/.well-known/openid-configuration", &disc); err != nil {
		return nil, fmt.Errorf("discovery: %w", err)
	}
	var jwks struct {
		Keys []struct {
			Kid, Kty, N, E string
		} `json:"keys"`
	}
	if err := v.getJSON(ctx, disc.JwksURI, &jwks); err != nil {
		return nil, fmt.Errorf("jwks: %w", err)
	}
	v.keys = map[string]*rsa.PublicKey{}
	for _, k := range jwks.Keys {
		if k.Kty != "RSA" {
			continue
		}
		nb, err1 := base64.RawURLEncoding.DecodeString(k.N)
		eb, err2 := base64.RawURLEncoding.DecodeString(k.E)
		if err1 != nil || err2 != nil {
			continue
		}
		v.keys[k.Kid] = &rsa.PublicKey{N: new(big.Int).SetBytes(nb), E: int(new(big.Int).SetBytes(eb).Int64())}
	}
	v.fetched = time.Now()
	if k, ok := v.keys[kid]; ok {
		return k, nil
	}
	return nil, fmt.Errorf("unknown key id %q", kid)
}

func (v *Verifier) getJSON(ctx context.Context, url string, out any) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return err
	}
	res, err := v.Client.Do(req)
	if err != nil {
		return err
	}
	defer res.Body.Close()
	if res.StatusCode != 200 {
		return fmt.Errorf("http %d", res.StatusCode)
	}
	return json.NewDecoder(res.Body).Decode(out)
}
