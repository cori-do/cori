# Licensing

Cori's open source core is under the Apache License 2.0 ([LICENSE](LICENSE)), with its [NOTICE](NOTICE). What
`cori init` copies into your apps and what your coding agent installs is under MIT No Attribution (MIT-0) instead: what
Cori writes into your repository is yours, with no notice to keep.

"The Cori Authors", in the copyright notices, means the people listed in [AUTHORS](AUTHORS) and everyone whose
contribution this repository's history shows.

| Path | Licence | Why |
|---|---|---|
| everything not listed below | Apache-2.0 ([LICENSE](LICENSE)) | the platform: server, workers, command line, console, SDKs, the plugin's manifests |
| `packages/client/` | MIT-0 ([LICENSE](packages/client/LICENSE)) | `cori init` copies it into every app (`front/vendor/cori-client/`) |
| `templates/`, `crates/cori-cli/front-template/` | MIT-0 (text below) | `cori init` copies them into new apps |
| `skills/` | MIT-0 ([LICENSE](skills/LICENSE)) | the plugin's skills, installed into your AI agent |

`packages/sdk/` (the step SDK your workflows import) and `sdks/` (the Go and .NET helpers your native activity workers
link) keep a copy of the Apache licence of their own ([LICENSE](packages/sdk/LICENSE), [LICENSE](sdks/LICENSE)), for
when they are published on their own.

Source files name their licence on an `SPDX-License-Identifier` line (`Apache-2.0` or `MIT-0`); a file without one is
under the licence of its path in the table above.

Releases up to v0.11.0 were published under the MIT License, and stay under it. The Apache License 2.0 applies from
the next release.

The released `cori` binary (`@cori-do/cli` on npm) also contains the open source software it is built with: the Rust
crates, and the JavaScript the console embeds. Each npm package carries Cori's LICENSE and NOTICE, and those crates'
and packages' licences in THIRD-PARTY-NOTICES.md (`scripts/cli-notices.sh`); `packages/cli/about.toml` lists the
licences a crate may have, and CI fails on another.

`templates/` and `crates/cori-cli/front-template/` hold no licence file, because `cori init` copies those directories
whole into new apps. Their licence text:

```text
MIT No Attribution

Copyright 2026 The Cori Authors

Permission is hereby granted, free of charge, to any person obtaining a copy of this
software and associated documentation files (the "Software"), to deal in the Software
without restriction, including without limitation the rights to use, copy, modify,
merge, publish, distribute, sublicense, and/or sell copies of the Software, and to
permit persons to whom the Software is furnished to do so.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED,
INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT
HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```
