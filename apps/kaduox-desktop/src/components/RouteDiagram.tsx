import type { RouteNode } from '../lib/types'

export function endpointLabel(host: string, port: number) {
  return (host.includes(':') && !host.startsWith('[') ? '[' + host + ']' : host) + ':' + port
}

export function RouteDiagram({ route, reverse = false }: { route: RouteNode[]; reverse?: boolean }) {
  const ordered = reverse ? [...route].reverse() : route
  return <ol className="forward-chain" aria-label={reverse ? '反向数据链路' : '连接链路'}>
    {ordered.map((node, index) => <li key={index} className={'chain-machine ' + node.role}>
      <span className="machine-number">{index + 1}</span>
      <div><small>{node.role === 'local' ? '本机客户端' : node.role === 'jump' ? 'SSH 跳板' : 'SSH 目标'}</small><strong>{node.alias}</strong><code>{endpointLabel(node.host, node.port)}</code><span>用户：{node.username || '未提供'}</span></div>
    </li>)}
  </ol>
}
