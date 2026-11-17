// SPDX-License-Identifier: Apache-2.0
// The envelope Cori's engine sends (sdks/testdata/native_envelope.json, which the engine's own tests check it produces)
// decodes into Cori.Envelope<T> through Temporal's default data converter, as a worker receives it, and the connector's
// key (sdks/testdata/connector_key.hex) opens the run's token sealed in it, for that run only.
//   dotnet run --project sdks/dotnet/Cori.Activities.Check -- sdks/testdata/native_envelope.json   (exit 1 on a mismatch)
using System.Text.Json.Serialization;
using Google.Protobuf;
using Temporalio.Api.Common.V1;
using Temporalio.Converters;

var path = args.Length > 0 ? args[0] : "sdks/testdata/native_envelope.json";
var payload = new Payload { Data = ByteString.CopyFrom(File.ReadAllBytes(path)) };
payload.Metadata["encoding"] = ByteString.CopyFromUtf8("json/plain");
var env = DataConverter.Default.PayloadConverter.ToValue<Cori.Envelope<RefundInput>>(payload);

var problems = new List<string>();
void Expect(bool ok, string what) { if (!ok) problems.Add(what); }
var c = env.Cori;
Expect(c.RunId == "refund_request-0001" && c.Workflow == "refund_request" && c.Step == "refund" && c.Attempt == 1, $"run context: {c}");
Expect(c.IdempotencyKey == "refund_request-0001:refund" && !string.IsNullOrEmpty(c.PrincipalToken), $"keys: {c}");
Expect(c.User.Sub == "alice" && c.HasRole("support_agent") && c.User.Email == "alice@acme.example", $"user: {c.User}");
Expect(env.Input.InvoiceId == "INV-42" && env.Input.AmountEur == 15.5m && env.Input.Reason == "charged twice", $"input: {env.Input}");
var key = Convert.FromHexString(File.ReadAllText(Path.Combine(Path.GetDirectoryName(path) ?? ".", "connector_key.hex")).Trim());
Expect(c.Token(key) == "eyJhbGciOiJSUzI1NiJ9.e30.c2ln", "the connector's key opens the run's token");
bool Refused(Action open) { try { open(); return false; } catch (System.Security.Cryptography.CryptographicException) { return true; } }
Expect(Refused(() => Cori.SealedToken.Open(c.PrincipalToken!, "refund_request-0002", key)), "a sealed token opens for its own run only");
var other = (byte[])key.Clone(); other[0] ^= 1;
Expect(Refused(() => c.Token(other)), "nor with another key");
if (problems.Count > 0) { foreach (var p in problems) Console.Error.WriteLine($"envelope mismatch: {p}"); return 1; }
Console.WriteLine("the engine's envelope decodes into Cori.Envelope<T>");
return 0;

record RefundInput(
    [property: JsonPropertyName("invoice_id")] string InvoiceId,
    [property: JsonPropertyName("amount_eur")] decimal AmountEur,
    [property: JsonPropertyName("reason")] string Reason);
