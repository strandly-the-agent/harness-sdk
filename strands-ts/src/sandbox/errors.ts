/**
 * Error types for sandbox command and code execution.
 *
 * These are runtime throwables raised by sandbox execution; consumers can
 * branch on them via `instanceof` to distinguish timeouts from aborts.
 */

/**
 * Output captured from a process up to the point it was killed.
 */
export interface PartialExecution {
  /** Standard output captured before the kill. */
  stdout: string
  /** Standard error captured before the kill. */
  stderr: string
}

/**
 * Thrown by sandbox execution when the configured `timeout` elapses.
 *
 * Carries whatever the process wrote before it was killed so callers can surface
 * partial output; empty when the sandbox cannot report what was captured.
 */
export class SandboxTimeoutError extends Error {
  readonly stdout: string
  readonly stderr: string

  constructor(seconds: number, partial?: PartialExecution) {
    super(`Execution timed out after ${seconds} seconds`)
    this.name = 'SandboxTimeoutError'
    this.stdout = partial?.stdout ?? ''
    this.stderr = partial?.stderr ?? ''
  }
}

/**
 * Thrown by sandbox execution when the abort signal fires.
 */
export class SandboxAbortError extends Error {
  constructor() {
    super('Execution aborted')
    this.name = 'SandboxAbortError'
  }
}

/**
 * Thrown by {@link Sandbox.listFiles} when the path does not exist, distinguishing
 * genuine absence from permission or transport failures (which throw plain errors).
 */
export class SandboxPathNotFoundError extends Error {
  constructor(path: string) {
    super(`Path not found: ${path}`)
    this.name = 'SandboxPathNotFoundError'
  }
}
