# @cori-do/sdk

TypeScript SDK to structure [Cori](https://github.com/cori-do/cori) workflow steps.


## Install

```bash
pnpm add @cori-do/sdk zod
```

## Usage (preview)

```ts
import { step } from "@cori-do/sdk";

export default step.code({
  description: "Square a number",
  // input/output zod schemas, run() function — add the rest of the step here.
});
```
