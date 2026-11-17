# Security

Cori runs other people's code and holds their data and credentials, so we take reports seriously and thank the people
who make them.

## Reporting a vulnerability

Please report it privately, through GitHub: **Security › Report a vulnerability** on
[cori-do/cori](https://github.com/cori-do/cori/security/advisories/new). Do not open a public issue, pull request or
discussion for it.

A useful report says what is affected (this repository's code, the `@cori-do/cli` package, or Cori Cloud), how to
reproduce it, and what an attacker gains. We confirm that we received it, keep you informed while we fix it, and credit
you in the advisory if you wish.

## Scope

- The code in this repository, including the released command line (`@cori-do/cli` on npm) and the plugin for AI agents.
- Cori Cloud: `app.cori.do`, its sign-in pages, and apps' fronts on `*.cori.run`.

When you test Cori Cloud, use an organization of your own, never touch other people's data, and do not degrade the
service for others (no denial of service, no automated scanning at volume, no social engineering).

## Supported versions

Fixes land on `main` and ship in the next release of `@cori-do/cli`; Cori Cloud runs the latest. Install the latest
release (`npm install -g @cori-do/cli@latest`) to get them.
