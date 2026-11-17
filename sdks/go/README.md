# cori (Go helpers for native activities)

```go
import cori "github.com/cori-do/cori/sdks/go"

type IssueRefundInput struct{ InvoiceID string `json:"invoice_id"`; AmountEur float64 `json:"amount_eur"` }
type IssueRefundOutput struct{ RefundID string `json:"refund_id"` }

var verifier = cori.NewVerifier("https://sso.acme.example/realms/acme", "billing-activities")

func IssueRefund(ctx context.Context, env cori.Envelope[IssueRefundInput]) (IssueRefundOutput, error) {
    token, err := env.Cori.Token() // sealed for this worker: CORI_CONNECTOR_KEY opens it
    if err != nil {
        return IssueRefundOutput{}, cori.NonRetryable("Forbidden", err)
    }
    if _, err := verifier.Verify(ctx, token); err != nil || !env.Cori.HasRole("support_agent", "support_lead") {
        return IssueRefundOutput{}, cori.NonRetryable("Forbidden", fmt.Errorf("caller may not refund"))
    }
    id, err := billing.Refund(ctx, env.Input.InvoiceID, env.Input.AmountEur, env.Cori.IdempotencyKey)
    return IssueRefundOutput{RefundID: id}, err
}
```

Register the activity under the name declared in `activities.yaml` (`worker.RegisterActivityWithOptions(IssueRefund, activity.RegisterOptions{Name: "IssueRefund"})`),
poll the declared task queue, and declare the connector once: `cori connectors add activities --yaml ./activities.yaml`.

The run's token reaches the activity sealed with the connector's key, since Temporal keeps the envelope in the run's
history: an admin prints the key with `cori connectors key <name>`, and the worker gets it as `CORI_CONNECTOR_KEY`.
`Token()` opens it for the envelope's own run (`OpenToken` takes the key explicitly).
