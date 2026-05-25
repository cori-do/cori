# @cori/sdk

TypeScript SDK for authoring [Cori](https://github.com/cori-do/cori) workflow steps.

> **Status:** v0.1.0-dev

## Install

```bash
pnpm add @cori/sdk zod
```

## Usage (preview)

```ts
import { step } from "@cori/sdk";

export default step.code({
  description: "Square a number",
  // input/output zod schemas, run() function — add the rest of the step here.
});
```
