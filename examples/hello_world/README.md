# hello_world

The bundled `cori demo` workflow. Three steps, zero credentials, zero cost.

```
$ cori demo
Running hello_world...
✓ step 1: fetch_quote (cli, 0.4s, €0.00)
✓ step 2: count_words (code, 0.0s, €0.00)
✓ step 3: format     (code, 0.0s, €0.00)

  "The best way out is always through." — Robert Frost (8 words)

Total: 0.5s, €0.00.
```

The workflow source is embedded into the `cori` binary at compile time and
extracted to `~/.cori/runbooks/hello_world/` the first time you run
`cori demo`. You can edit those files and re-run `cori workflows register`
to experiment with your own version.
