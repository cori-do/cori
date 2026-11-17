// SPDX-License-Identifier: Apache-2.0
package cori

import (
	"encoding/hex"
	"os"
	"strings"
	"testing"

	commonpb "go.temporal.io/api/common/v1"
	"go.temporal.io/sdk/converter"
)

type refundInput struct {
	InvoiceID string  `json:"invoice_id"`
	AmountEur float64 `json:"amount_eur"`
	Reason    string  `json:"reason"`
}

// The envelope Cori's engine sends (../testdata/native_envelope.json, which the engine's own tests check it produces)
// decodes into Envelope through the SDK's default data converter, as a worker receives it.
func TestEnvelopeDecodesWhatTheEngineSends(t *testing.T) {
	b, err := os.ReadFile("../testdata/native_envelope.json")
	if err != nil {
		t.Fatal(err)
	}
	var env Envelope[refundInput]
	p := &commonpb.Payload{Metadata: map[string][]byte{"encoding": []byte("json/plain")}, Data: b}
	if err := converter.GetDefaultDataConverter().FromPayload(p, &env); err != nil {
		t.Fatal(err)
	}
	c := env.Cori
	if c.RunID != "refund_request-0001" || c.Workflow != "refund_request" || c.Step != "refund" || c.Attempt != 1 || c.IdempotencyKey != "refund_request-0001:refund" || c.PrincipalToken == "" {
		t.Fatalf("context: %+v", c)
	}
	if c.User.Sub != "alice" || !c.HasRole("support_agent") || c.User.Email != "alice@acme.example" {
		t.Fatalf("user: %+v", c.User)
	}
	if env.Input.InvoiceID != "INV-42" || env.Input.AmountEur != 15.5 || env.Input.Reason != "charged twice" {
		t.Fatalf("input: %+v", env.Input)
	}
}

// The run's token is sealed for the connector's worker: its key (../testdata/connector_key.hex) opens it, for its run only.
func TestTheConnectorKeyOpensTheRunsToken(t *testing.T) {
	b, _ := os.ReadFile("../testdata/native_envelope.json")
	var env Envelope[refundInput]
	p := &commonpb.Payload{Metadata: map[string][]byte{"encoding": []byte("json/plain")}, Data: b}
	if err := converter.GetDefaultDataConverter().FromPayload(p, &env); err != nil {
		t.Fatal(err)
	}
	hexKey, _ := os.ReadFile("../testdata/connector_key.hex")
	t.Setenv("CORI_CONNECTOR_KEY", strings.TrimSpace(string(hexKey)))
	if tok, err := env.Cori.Token(); err != nil || tok != "eyJhbGciOiJSUzI1NiJ9.e30.c2ln" {
		t.Fatalf("token: %q %v", tok, err)
	}
	key, _ := hex.DecodeString(strings.TrimSpace(string(hexKey)))
	if _, err := OpenToken(env.Cori.PrincipalToken, "refund_request-0002", key); err == nil {
		t.Fatal("a sealed token opened for another run")
	}
	key[0] ^= 1
	if _, err := OpenToken(env.Cori.PrincipalToken, env.Cori.RunID, key); err == nil {
		t.Fatal("a sealed token opened with another key")
	}
	t.Setenv("CORI_CONNECTOR_KEY", "")
	if _, err := env.Cori.Token(); err == nil {
		t.Fatal("a sealed token opened without the key")
	}
}
