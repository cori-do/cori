---
name: cori-start
description: Take someone from nothing to their first Cori app, open in their browser — signing in to Cori, an app from the starter template, a first release and its preview link. Use when the user wants to start with Cori, try Cori, set Cori up, or has just installed the Cori plugin, connector or skills.
---

# Start with Cori

The person in front of you may not be technical. Take them from nothing to a first Cori app they can open in their
browser, in the order below. Say in one short, plain line what you are doing at each step (no jargon: "Signing you in
to Cori", "Creating your app"). When a step fails, say what went wrong in plain words and what to do, and stop there.

The app id is the one the user gave (`$ARGUMENTS`, when your host passes one), else `my-team`: letters, digits, `-`
and `_` (it becomes part of the app's address).

**Two ways to reach Cori.** Use the Cori tools (the `cori` MCP server: `list_apps`, `create_app`, `login`, `preview`)
whenever they are connected: their sign-in is already done. Use the `cori` command line only for what the tools cannot
do (writing the app's folder, releasing it), and only where you can run commands.

1. **Sign in.** Call the `list_apps` tool.
   - It works: go on.
   - The tools are there but ask for authentication: ask the user to connect Cori in their app (in Claude Code: `/mcp`,
     choose `cori`, then sign in). Their browser opens on Cori's sign-in page, where a new user creates an account in a
     minute. Then call `list_apps` again.
   - There are no Cori tools at all: you can still go on with the command line (step 2), which signs in with
     `cori login https://app.cori.do` (it opens the browser). Tell the user that adding the Cori connector
     (https://app.cori.do/mcp) makes the next times smoother.
   - Never ask for a password, a code or a token in the conversation.
2. **The command line** (only where you can run commands). Run `cori --version`. If `cori` is missing, do not install
   anything globally: write `npx -y @cori-do/cli@latest` wherever the steps below say `cori` (it needs Node.js; if
   `npx` is missing too, ask the user to install Node.js from https://nodejs.org, the LTS button, then try again). It
   runs on macOS and Linux; on Windows, it needs WSL.
3. **The app.** Call `create_app` with `{"id": "<app id>", "template": "team-requests"}`. If the id is taken, pick
   another one (`list_apps` shows the user's apps) or ask the user. Without the tools: `cori init <app id> --template
   team-requests` after `cori login`.
4. **A session for the command line.** Call `login` with `{"app": "<app id>"}`. In the current folder (or the one the
   user named), run the `bootstrap` command it returns, as it is: it writes the folder `<app id>/` from the template,
   with the session in `<app id>/.cori/session`. The session is short-lived and limited to this app; when a later
   command says it expired, call `login` again and write its `session_toml` to `<app id>/.cori/session`.
5. **The first release.** In `<app id>/`, run `cori deploy`. The app's screens are already built, so this needs nothing
   else. Give the user the preview link it prints, and suggest they open it.
6. **What to try**, in three short lines:
   - The preview runs on practice data of its own. Nothing is live until a person makes the release live in the Cori
     console; you cannot do that for them.
   - A new request up to 100 € is approved at once by a rule; above that, or for travel, it waits in Approvals for a
     manager. As the app's owner, the user is both a member and a manager, so they can try both sides.
   - The AI step runs on the organization's model or, until someone adds one in the console (Settings › AI models), on
     Cori's free starter model: a few calls a day, and nothing confidential.
7. **Next.** Offer to make the app theirs: what it should keep track of, who approves what, what the screens show. Then
   read the app's `AGENTS.md` and follow the `cori-build-app` skill.

**In a chat app without commands** (claude.ai, ChatGPT, Claude or ChatGPT on the phone): steps 2, 4 and 5 cannot run.
Do steps 1 and 3, then say that the first release needs a desktop assistant that can run commands (Claude Code, Claude
Cowork, Codex, Cursor…), and once a release exists, show it with the `preview` tool.
