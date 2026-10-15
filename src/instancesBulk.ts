import type { DshInstance, InstanceStatus } from '@/api/types'

/** The rows the board is showing, split by what the two bulk buttons would do. */
export interface BulkScope {
  /** Rows 「全部启动」 would start. */
  start: DshInstance[]
  /** Rows 「全部停止」 would stop. */
  stop: DshInstance[]
  /** Rows left alone because the launcher cannot reach their machine. */
  unreachable: DshInstance[]
}

/**
 * What 「全部启动」 and 「全部停止」 act on: the rows the board is showing and
 * nothing else. One definition, so a filter that says 「只看这台机器」 and the
 * buttons below it cannot disagree about the scope.
 *
 * 「主机不可达」 is a third answer, not a kind of 「已停止」: the launcher cannot
 * say whether that machine already runs the instance, so starting one there is
 * the click that puts a second DSH on a server, and the row's own action for
 * that state is 「重试」 (ADR-0007). Those rows are left alone and reported
 * instead of attempted.
 */
export function bulkScope(
  visible: DshInstance[],
  statusOf: (id: string) => InstanceStatus,
): BulkScope {
  const scope: BulkScope = { start: [], stop: [], unreachable: [] }
  for (const inst of visible) {
    const state = statusOf(inst.id).state
    if (state === 'running') scope.stop.push(inst)
    else if (state === 'unreachable') scope.unreachable.push(inst)
    else if (state !== 'starting') scope.start.push(inst)
  }
  return scope
}
