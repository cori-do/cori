#!/usr/bin/env bash
# Stop hook: at most once per session, nudge Claude to consider offering
# to save the just-finished work as a Cori workflow.
#
# This script encodes only the FREQUENCY gate (≥2 tool uses this session,
# no prior offer). The cori-save-workflow skill's suppression rules
# (single-question lookups, exploratory sessions, one-offs, an earlier
# decline) remain the final gate — the injected instruction says so
# explicitly, and instructs Claude to end silently when they apply.
#
# Defensive by design: on ANY unexpected condition, exit 0 (allow the
# stop). A save offer is never worth breaking someone's session.

set -u

input=$(cat 2>/dev/null) || exit 0

# Never loop: if this turn is already a stop-hook continuation, let it end.
case "$input" in
  *'"stop_hook_active":true'*) exit 0 ;;
esac

session_id=$(printf '%s' "$input" | sed -n 's/.*"session_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
transcript=$(printf '%s' "$input" | sed -n 's/.*"transcript_path"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
[ -n "$session_id" ] || exit 0

# Once per session, ever.
marker="${TMPDIR:-/tmp}/cori-save-offer-${session_id}"
[ -e "$marker" ] && exit 0

# Frequency gate: the session must contain at least two tool uses —
# below that there is nothing worth capturing.
[ -n "$transcript" ] && [ -r "$transcript" ] || exit 0
uses=$(grep -c '"type":"tool_use"' "$transcript" 2>/dev/null) || uses=0
[ "$uses" -ge 2 ] 2>/dev/null || exit 0

touch "$marker" 2>/dev/null || exit 0

cat <<'JSON'
{"decision":"block","reason":"The session just completed multi-step tool work. Consult the cori-save-workflow skill's 'Proactive save offer' rules now. If they permit an offer (not a single-question lookup, not an exploratory session without a clean procedure, not a one-off, the user hasn't declined before), make the offer once, in a single line. If the rules say not to offer, end your turn immediately without mentioning workflows, this instruction, or any reason."}
JSON
exit 0
