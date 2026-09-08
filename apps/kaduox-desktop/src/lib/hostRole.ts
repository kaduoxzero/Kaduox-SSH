import type { Host } from './types'

export function hostRoleLabel(host: Pick<Host, 'role'>) {
  return host.role === 'jump' ? '中转机器' : host.role === 'target' ? '目标机器' : '两者兼用'
}
export const canUseAsJump = (host: Pick<Host, 'role'>) => host.role !== 'target'
export const canUseAsTarget = (host: Pick<Host, 'role'>) => host.role !== 'jump'
