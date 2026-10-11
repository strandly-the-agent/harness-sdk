/**
 * Proactive restart timer for bidirectional streaming.
 *
 * `RestartTimer` fires a warning then a deadline callback at caller-supplied offsets; it holds no
 * restart policy. `resolveDeadlineS` reads the deadline from a provider's declared
 * `ConnectionConfig`.
 *
 * @internal
 */

import { logger } from '../../logging/index.js'
import type { ConnectionConfig } from '../models/configs.js'

/** Injectable async sleep, in seconds. */
export type SleepFn = (seconds: number) => Promise<void>

/**
 * Resolve the proactive restart deadline in seconds from a connection config.
 *
 * @param connectionConfig - Provider-declared restart timing.
 * @returns `restartAfterS` if declared and positive, else `undefined` (no proactive timer).
 * @internal
 */
export function resolveDeadlineS(connectionConfig: ConnectionConfig): number | undefined {
  const restartAfterS = connectionConfig.restartAfterS
  if (restartAfterS === undefined || restartAfterS <= 0) {
    return undefined
  }
  return restartAfterS
}

/** Default sleep backed by `setTimeout`, abortable through the supplied signal. */
function defaultSleep(seconds: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal.aborted) {
      reject(signal.reason)
      return
    }
    const timer = setTimeout(() => {
      signal.removeEventListener('abort', onAbort)
      resolve()
    }, seconds * 1000)
    function onAbort(): void {
      clearTimeout(timer)
      reject(signal.reason)
    }
    signal.addEventListener('abort', onAbort, { once: true })
  })
}

/**
 * Fire a warning then a deadline callback ahead of a provider's connection limit.
 *
 * The clock is injectable so tests can drive timing without wall time.
 *
 * @internal
 */
export class RestartTimer {
  private readonly _onWarning: (timeLeftS: number) => Promise<void>
  private readonly _onDeadline: () => Promise<void>
  private readonly _sleep: SleepFn | undefined
  private _controller: AbortController | undefined
  /** The in-flight cycle, exposed for tests to await. */
  private _task: Promise<void> | undefined

  /**
   * @param onWarning - Called with seconds-left when the warning lead elapses.
   * @param onDeadline - Called when the restart deadline elapses.
   * @param sleep - Injectable async sleep (for tests). Defaults to a `setTimeout` sleep.
   */
  constructor(onWarning: (timeLeftS: number) => Promise<void>, onDeadline: () => Promise<void>, sleep?: SleepFn) {
    this._onWarning = onWarning
    this._onDeadline = onDeadline
    this._sleep = sleep
  }

  /**
   * Arm the warning and deadline timers, cancelling any previously armed cycle.
   *
   * @param deadlineS - Seconds from now until the deadline callback fires.
   * @param warningLeadS - Seconds before the deadline to fire the warning callback.
   */
  arm(deadlineS: number, warningLeadS: number): void {
    this.cancel()
    const controller = new AbortController()
    this._controller = controller
    this._task = this._run(deadlineS, warningLeadS, controller)
    logger.debug(
      `deadline_s=<${deadlineS.toFixed(1)}>, warning_lead_s=<${warningLeadS.toFixed(1)}> | proactive restart timer armed`
    )
  }

  /** Cancel the armed timer, if any. Safe to call when idle. */
  cancel(): void {
    if (this._controller !== undefined) {
      this._controller.abort(new Error('restart timer cancelled'))
      this._controller = undefined
    }
  }

  /**
   * Sleep until the warning lead, fire the warning, then fire the deadline.
   *
   * The warning fires `warningLeadS` before the deadline. When the lead is zero or exceeds the
   * deadline, the warning is emitted immediately and the remaining wait runs down to the deadline.
   * The deadline countdown continues while warning delivery is backpressured, but the deadline
   * callback still follows the warning.
   */
  private async _run(deadlineS: number, warningLeadS: number, controller: AbortController): Promise<void> {
    const sleep = (seconds: number): Promise<void> =>
      this._sleep ? this._sleep(seconds) : defaultSleep(seconds, controller.signal)
    const warningAtS = Math.max(deadlineS - warningLeadS, 0)

    try {
      await sleep(warningAtS)
      if (controller.signal.aborted) return

      const timeLeftS = deadlineS - warningAtS
      // Start the deadline countdown before awaiting the warning so backpressure on warning
      // delivery does not push the deadline out.
      const deadlineSleep = sleep(timeLeftS)
      deadlineSleep.catch(() => {})

      await this._onWarning(timeLeftS)
      await deadlineSleep
      if (controller.signal.aborted) return

      // Detach before the callback re-arms this timer; cancelling a live self-reference
      // would abort the restart the callback runs.
      this._controller = undefined
      await this._onDeadline()
    } catch (error) {
      if (controller.signal.aborted) return
      logger.warn(`error=<${String(error)}> | proactive restart timer failed`)
    }
  }
}
