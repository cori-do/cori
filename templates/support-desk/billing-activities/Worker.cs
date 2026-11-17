// SPDX-License-Identifier: MIT-0
// The billing team's worker: the official Temporal .NET SDK with Cori's helpers for the envelope and the principal check
// (sdks/dotnet, package Cori.Activities). `IBillingService` is the billing system's own client (not shown).
using System.Text.Json.Serialization;
using Cori;
using Temporalio.Activities;
using Temporalio.Client;
using Temporalio.Worker;

public record IssueRefundInput(
    [property: JsonPropertyName("invoice_id")] string InvoiceId,
    [property: JsonPropertyName("amount_eur")] decimal AmountEur,
    [property: JsonPropertyName("reason")] string Reason);
public record IssueRefundOutput([property: JsonPropertyName("refund_id")] string RefundId);

public class BillingActivities(PrincipalVerifier verifier, IBillingService billing)
{
    [Activity("IssueRefund")]
    public async Task<IssueRefundOutput> IssueRefund(Envelope<IssueRefundInput> env)
    {
        // the token Cori exchanged for this connector's audience (activities.yaml `auth`), checked against the org's IdP
        try { await verifier.VerifyAsync(env.Cori.PrincipalToken); }
        catch (Exception e) { throw CoriErrors.NonRetryable("Forbidden", $"principal token refused: {e.Message}"); }
        if (!env.Cori.HasRole("support_agent", "support_lead"))
            throw CoriErrors.NonRetryable("Forbidden", "the caller may not refund");

        var ctx = ActivityExecutionContext.Current;
        ctx.Heartbeat("starting");   // activities.yaml declares a 20 s heartbeat
        var refundId = await billing.RefundAsync(env.Input.InvoiceId, env.Input.AmountEur, env.Input.Reason,
                                                 idempotencyKey: env.Cori.IdempotencyKey, ct: ctx.CancellationToken);
        return new IssueRefundOutput(refundId);
    }
}

// Program.cs
var verifier = new PrincipalVerifier("https://sso.acme.internal/realms/acme", audience: "billing-activities");
var client = await TemporalClient.ConnectAsync(new("temporal.acme.internal:7233") { Namespace = "acme" /* mTLS or API key per connector */ });
using var worker = new TemporalWorker(client,
    new TemporalWorkerOptions(taskQueue: "cori.acme.native.billing").AddAllActivities(new BillingActivities(verifier, billing)));
await worker.ExecuteAsync(cts.Token);
