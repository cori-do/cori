# Cori.Activities (.NET helpers for native activities)

```csharp
using Cori;

public record IssueRefundInput([property: JsonPropertyName("invoice_id")] string InvoiceId, [property: JsonPropertyName("amount_eur")] double AmountEur);
public record IssueRefundOutput([property: JsonPropertyName("refund_id")] string RefundId);

public class Billing
{
    static readonly PrincipalVerifier Verifier = new("https://sso.acme.example/realms/acme", "billing-activities");

    [Activity("IssueRefund")]
    public async Task<IssueRefundOutput> IssueRefund(Envelope<IssueRefundInput> env)
    {
        await Verifier.VerifyAsync(env.Cori.Token());   // sealed for this worker: CORI_CONNECTOR_KEY opens it
        if (!env.Cori.HasRole("support_agent", "support_lead")) throw CoriErrors.NonRetryable("Forbidden", "caller may not refund");
        var id = await Refund(env.Input.InvoiceId, env.Input.AmountEur, env.Cori.IdempotencyKey);
        return new IssueRefundOutput(id);
    }
}
```

Register the activity under the name declared in `activities.yaml`, poll the declared task queue, and declare the connector once:
`cori connectors add activities --yaml ./activities.yaml`.

The run's token reaches the activity sealed with the connector's key, since Temporal keeps the envelope in the run's
history: an admin prints the key with `cori connectors key <name>`, and the worker gets it as `CORI_CONNECTOR_KEY`.
`Token()` opens it for the envelope's own run (`SealedToken.Open` takes the key explicitly).
