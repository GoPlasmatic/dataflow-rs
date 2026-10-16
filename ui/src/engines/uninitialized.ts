/**
 * Rethrows what a call into `@goplasmatic/dataflow-wasm` threw, naming the
 * cause when it was a call before `init()`.
 *
 * Before `init()` the generated wrapper reads its export off an instance
 * that is still undefined, which surfaces as a `TypeError` that names no
 * cause (`Cannot read properties of undefined (reading '__wbindgen_free')`).
 * That becomes an `Error` saying what to do, with the original as its
 * `cause`. Anything else is the engine's own error, rethrown as it is.
 *
 * `action` completes "must be initialized (`await init()`) before ...".
 */
export function rethrowUninitialized(err: unknown, action: string): never {
  if (err instanceof TypeError) {
    throw new Error(
      `@goplasmatic/dataflow-wasm must be initialized (\`await init()\`) before ${action}.`,
      { cause: err },
    );
  }
  throw err;
}
