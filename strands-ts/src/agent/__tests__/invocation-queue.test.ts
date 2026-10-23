import { describe, expect, it } from 'vitest'
import { InvocationQueue } from '../invocation-queue.js'
import { PendingInvocationCancelledError } from '../../errors.js'

describe('InvocationQueue', () => {
  it('counts queued entries and reports a stop request while a stopPrevious entry waits', () => {
    const queue = new InvocationQueue()
    void queue.wait({ mode: 'queue' }).catch(() => {})
    expect(queue.hasStopRequest()).toBe(false)
    void queue.wait({ mode: 'stopPrevious' }).catch(() => {})

    expect(queue.size).toBe(2)
    expect(queue.hasStopRequest()).toBe(true)
    queue.handoff()
    expect(queue.hasStopRequest()).toBe(false)
  })

  it('resolves waiters FIFO on handoff', async () => {
    const queue = new InvocationQueue()
    const order: string[] = []
    const first = queue.wait({ mode: 'queue' }).then(() => order.push('a'))
    const second = queue.wait({ mode: 'queue' }).then(() => order.push('b'))

    expect(queue.handoff()).toBe(true)
    await first
    expect(queue.handoff()).toBe(true)
    await second
    expect(order).toEqual(['a', 'b'])
    expect(queue.size).toBe(0)
  })

  it('returns false from handoff when empty', () => {
    expect(new InvocationQueue().handoff()).toBe(false)
  })

  it('inserts cancelPrevious entries ahead of waiting ones', async () => {
    const queue = new InvocationQueue()
    const order: string[] = []
    const normal = queue.wait({ mode: 'queue' }).then(() => order.push('normal'))
    const urgent = queue.wait({ mode: 'cancelPrevious' }).then(() => order.push('urgent'))

    queue.handoff()
    await urgent
    queue.handoff()
    await normal
    expect(order).toEqual(['urgent', 'normal'])
  })

  it('a cancelPrevious entry displaces queued cancelPrevious predecessors but not plain ones', async () => {
    const queue = new InvocationQueue()
    const plain = queue.wait({ mode: 'queue' })
    const older = queue.wait({ mode: 'cancelPrevious' })
    const newer = queue.wait({ mode: 'cancelPrevious' })

    await expect(older).rejects.toThrow(PendingInvocationCancelledError)
    expect(queue.size).toBe(2)
    queue.handoff()
    await newer
    queue.handoff()
    await plain
  })

  it('rejects immediately when the cancelSignal is already aborted', async () => {
    const queue = new InvocationQueue()
    const controller = new AbortController()
    controller.abort()
    await expect(queue.wait({ mode: 'queue', cancelSignal: controller.signal })).rejects.toThrow(
      PendingInvocationCancelledError
    )
    expect(queue.size).toBe(0)
  })

  it('removes the entry and rejects when the cancelSignal aborts while queued', async () => {
    const queue = new InvocationQueue()
    const controller = new AbortController()
    const waiting = queue.wait({ mode: 'queue', cancelSignal: controller.signal })
    controller.abort()
    await expect(waiting).rejects.toThrow(PendingInvocationCancelledError)
    expect(queue.size).toBe(0)
  })

  it('detaches the abort listener on handoff (a later abort does not reject)', async () => {
    const queue = new InvocationQueue()
    const controller = new AbortController()
    const waiting = queue.wait({ mode: 'queue', cancelSignal: controller.signal })
    queue.handoff()
    await expect(waiting).resolves.toBeUndefined()
    controller.abort()
  })

  it('notifies onEnqueue listeners when an entry enters the queue, including at the front', () => {
    const queue = new InvocationQueue()
    let notified = 0
    queue.onEnqueue(() => notified++)
    void queue.wait({ mode: 'queue' }).catch(() => {})
    expect(notified).toBe(1)
    void queue.wait({ mode: 'cancelPrevious' }).catch(() => {})
    expect(notified).toBe(2)
  })

  it('does not notify onEnqueue when a pre-aborted call is rejected without queueing', () => {
    const queue = new InvocationQueue()
    let notified = 0
    queue.onEnqueue(() => notified++)
    const aborted = new AbortController()
    aborted.abort()
    void queue.wait({ mode: 'queue', cancelSignal: aborted.signal }).catch(() => {})
    expect(notified).toBe(0)
  })

  it('stops notifying a detached onEnqueue listener', () => {
    const queue = new InvocationQueue()
    let notified = 0
    const detach = queue.onEnqueue(() => notified++)
    void queue.wait({ mode: 'queue' }).catch(() => {})
    detach()
    void queue.wait({ mode: 'queue' }).catch(() => {})
    expect(notified).toBe(1)
  })

  it('stopPrevious entries insert at the front and displace queued latest-wins predecessors of either mode', async () => {
    const queue = new InvocationQueue()
    const plain = queue.wait({ mode: 'queue' })
    const olderCancel = queue.wait({ mode: 'cancelPrevious' })
    const olderStop = queue.wait({ mode: 'stopPrevious' })
    const newest = queue.wait({ mode: 'stopPrevious' })

    await expect(olderCancel).rejects.toThrow(PendingInvocationCancelledError)
    await expect(olderStop).rejects.toThrow(PendingInvocationCancelledError)
    expect(queue.size).toBe(2)
    expect(queue.hasStopRequest()).toBe(true)
    queue.handoff()
    await newest
    queue.handoff()
    await plain
  })

  it('a cancelPrevious entry displaces a queued stopPrevious entry', async () => {
    const queue = new InvocationQueue()
    const stop = queue.wait({ mode: 'stopPrevious' })
    const cancel = queue.wait({ mode: 'cancelPrevious' })

    await expect(stop).rejects.toThrow(PendingInvocationCancelledError)
    expect(queue.size).toBe(1)
    queue.handoff()
    await cancel
  })
})
