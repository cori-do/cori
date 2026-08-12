export function assertEquals(
  actual: unknown,
  expected: unknown,
  message?: string,
) {
  const actualJson = JSON.stringify(actual);
  const expectedJson = JSON.stringify(expected);
  if (actualJson !== expectedJson) {
    throw new Error(
      message ??
        `assertEquals failed:\n  actual:   ${actualJson}\n  expected: ${expectedJson}`,
    );
  }
}

export function assert(condition: unknown, message = "assertion failed") {
  if (!condition) throw new Error(message);
}
