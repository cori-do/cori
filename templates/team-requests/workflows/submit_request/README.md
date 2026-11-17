# Submit a request

## Goal
Anyone on the team asks for something (equipment, software, travel, training). A model sorts the request and writes a
one-line summary; a rule in code approves amounts up to 100 €; anything above, and all travel, waits for a manager.
The outcome is recorded on the request in the store. The model never decides: it sorts, `route` decides.

## Dataflow
```
triage(llm) ─▶ route(code) ─▶ record(store) ─▶ approval{ [approve_request(human)] } ─▶ decide(code) ─▶ close(store)
```
`route` and `record` both depend on `triage` only, so they run side by side.

## Verification
- change review: requests `MayWrite` (createRequest, updateRequest), model `Prompt` (small tier)
- up to 100 € and not travel: no approval item, `decided_by: rule`; above 100 € or travel: an item in the managers'
  inbox, and the run waits up to 7 days (then `declined`, note "No decision within 7 days")
- the store refuses any other change of `status` than pending → approved | declined, and every write outside a run
  of this workflow (`writes: workflows`)
- locally: `cori run workflows/submit_request title="Second monitor" amount_eur=180 --decide approve_request='{"approved":true}'`
