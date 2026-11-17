// SPDX-License-Identifier: MIT-0
// Same connector, Go flavour: the official Temporal Go SDK with Cori's helpers for the envelope and the principal check
// (sdks/go). Either worker can serve the queue. `billing` is the billing system's own client (not shown).
package main

import (
	"context"
	"errors"

	cori "github.com/cori-do/cori/sdks/go"
	"go.temporal.io/sdk/activity"
	"go.temporal.io/sdk/client"
	"go.temporal.io/sdk/worker"
)

type IssueRefundInput struct {
	InvoiceID string  `json:"invoice_id"`
	AmountEur float64 `json:"amount_eur"`
	Reason    string  `json:"reason"`
}

type IssueRefundOutput struct {
	RefundID string `json:"refund_id"`
}

// The token Cori exchanged for this connector's audience (activities.yaml `auth`), checked against the org's IdP.
var verifier = cori.NewVerifier("https://sso.acme.internal/realms/acme", "billing-activities")

func IssueRefund(ctx context.Context, env cori.Envelope[IssueRefundInput]) (IssueRefundOutput, error) {
	if _, err := verifier.Verify(ctx, env.Cori.PrincipalToken); err != nil {
		return IssueRefundOutput{}, cori.NonRetryable("Forbidden", err)
	}
	if !env.Cori.HasRole("support_agent", "support_lead") {
		return IssueRefundOutput{}, cori.NonRetryable("Forbidden", errors.New("the caller may not refund"))
	}
	activity.RecordHeartbeat(ctx, "starting") // activities.yaml declares a 20 s heartbeat
	id, err := billing.Refund(ctx, env.Input.InvoiceID, env.Input.AmountEur, env.Input.Reason, env.Cori.IdempotencyKey)
	return IssueRefundOutput{RefundID: id}, err
}

func main() {
	c, _ := client.Dial(client.Options{HostPort: "temporal.acme.internal:7233", Namespace: "acme"})
	w := worker.New(c, "cori.acme.native.billing", worker.Options{})
	w.RegisterActivityWithOptions(IssueRefund, activity.RegisterOptions{Name: "IssueRefund"})
	_ = w.Run(worker.InterruptCh())
}
