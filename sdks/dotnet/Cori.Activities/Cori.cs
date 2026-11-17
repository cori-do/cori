// SPDX-License-Identifier: Apache-2.0
// Helpers for a Temporal .NET worker that serves Cori native activities.
using System.Security.Cryptography;
using System.Text;
using System.Text.Json.Serialization;
using Microsoft.IdentityModel.Protocols;
using Microsoft.IdentityModel.Protocols.OpenIdConnect;
using Microsoft.IdentityModel.Tokens;
using System.IdentityModel.Tokens.Jwt;
using Temporalio.Exceptions;

namespace Cori;

/// <summary>The run's user as Cori resolved it.</summary>
public sealed record Principal(
    [property: JsonPropertyName("sub")] string Sub,
    [property: JsonPropertyName("roles")] string[] Roles,
    [property: JsonPropertyName("email")] string? Email);

/// <summary>The `cori` half of the envelope: run, step, user, exchanged token (sealed for this connector's worker, since
/// Temporal keeps the envelope in the run's history: read it with <see cref="Token"/>), idempotency key.</summary>
public sealed record CoriContext(
    [property: JsonPropertyName("run_id")] string RunId,
    [property: JsonPropertyName("workflow")] string Workflow,
    [property: JsonPropertyName("version")] string Version,
    [property: JsonPropertyName("step")] string Step,
    [property: JsonPropertyName("attempt")] int Attempt,
    [property: JsonPropertyName("user")] Principal User,
    [property: JsonPropertyName("principal_token")] string? PrincipalToken,
    [property: JsonPropertyName("idempotency_key")] string IdempotencyKey)
{
    public bool HasRole(params string[] roles) => roles.Any(r => User.Roles.Contains(r));

    /// <summary>The run's token for this connector, opened with the connector's key (<paramref name="key"/>, else
    /// CORI_CONNECTOR_KEY in hex, from `cori connectors key &lt;name&gt;`); null when the envelope has none (a local run,
    /// a connector without an audience).</summary>
    public string? Token(byte[]? key = null)
    {
        if (PrincipalToken is null || !PrincipalToken.StartsWith(SealedToken.Prefix, StringComparison.Ordinal)) return PrincipalToken;
        key ??= HexKey(Environment.GetEnvironmentVariable("CORI_CONNECTOR_KEY"));
        return SealedToken.Open(PrincipalToken, RunId, key);
    }

    private static byte[] HexKey(string? hex)
    {
        try { var k = Convert.FromHexString((hex ?? "").Trim()); if (k.Length == 32) return k; } catch (FormatException) { }
        throw new CryptographicException("CORI_CONNECTOR_KEY is not this connector's key (64 hex digits: `cori connectors key <name>`)");
    }
}

/// <summary>A token Cori sealed for a connector's worker: base64url of nonce (12 bytes), ciphertext and tag, AES-256-GCM
/// under the connector's key, the run id as associated data (a token opens for its own run only).</summary>
public static class SealedToken
{
    public const string Prefix = "cori-sealed.v1.";

    public static string Open(string sealedToken, string runId, byte[] key)
    {
        if (!sealedToken.StartsWith(Prefix, StringComparison.Ordinal)) throw new CryptographicException("not a sealed token");
        var b64 = sealedToken[Prefix.Length..].Replace('-', '+').Replace('_', '/');
        var b = Convert.FromBase64String(b64 + (b64.Length % 4) switch { 2 => "==", 3 => "=", _ => "" });
        if (b.Length < 12 + 16) throw new CryptographicException("sealed token too short");
        var plain = new byte[b.Length - 12 - 16];
        using var gcm = new AesGcm(key, 16);
        gcm.Decrypt(b.AsSpan(0, 12), b.AsSpan(12, plain.Length), b.AsSpan(b.Length - 16), plain, Encoding.UTF8.GetBytes(runId));
        return Encoding.UTF8.GetString(plain);
    }
}

/// <summary>What every Cori-dispatched activity receives.</summary>
public sealed record Envelope<T>(
    [property: JsonPropertyName("input")] T Input,
    [property: JsonPropertyName("cori")] CoriContext Cori);

public static class CoriErrors
{
    /// <summary>An error Cori matches against `retry.non_retryable` in activities.yaml (e.g. "Forbidden").</summary>
    public static ApplicationFailureException NonRetryable(string errorType, string message) =>
        new(message, errorType: errorType, nonRetryable: true);
}

/// <summary>Verifies the exchanged principal token against the org's OIDC issuer (signature, expiry, issuer, audience).</summary>
public sealed class PrincipalVerifier
{
    private readonly ConfigurationManager<OpenIdConnectConfiguration> _config;
    private readonly string _issuer;
    private readonly string? _audience;

    public PrincipalVerifier(string issuer, string? audience = null)
    {
        _issuer = issuer.TrimEnd('/');
        _audience = audience;
        _config = new ConfigurationManager<OpenIdConnectConfiguration>($"{_issuer}/.well-known/openid-configuration", new OpenIdConnectConfigurationRetriever());
    }

    public async Task<System.Security.Claims.ClaimsPrincipal> VerifyAsync(string? token, CancellationToken ct = default)
    {
        if (string.IsNullOrEmpty(token)) throw new SecurityTokenException("no principal token in the envelope");
        var cfg = await _config.GetConfigurationAsync(ct);
        var parameters = new TokenValidationParameters
        {
            ValidIssuer = _issuer,
            IssuerSigningKeys = cfg.SigningKeys,
            ValidateAudience = _audience is not null,
            ValidAudience = _audience,
            ValidateLifetime = true,
        };
        return new JwtSecurityTokenHandler().ValidateToken(token, parameters, out _);
    }
}
