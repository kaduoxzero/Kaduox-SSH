import Check from 'lucide-react/dist/esm/icons/check'
import Info from 'lucide-react/dist/esm/icons/info'
import X from 'lucide-react/dist/esm/icons/x'
import XCircle from 'lucide-react/dist/esm/icons/circle-x'

import type { ToastMessage } from '../lib/types'

interface ToastRegionProps {
  messages: ToastMessage[]
  onDismiss: (id: number) => void
}

export function ToastRegion({ messages, onDismiss }: ToastRegionProps) {
  return (
    <div className="toast-region" aria-live="polite" aria-label="通知">
      {messages.map((message) => (
        <div className={`toast ${message.kind}`} key={message.id}>
          <span className="toast-icon">
            {message.kind === 'success' ? <Check size={15} /> : message.kind === 'error' ? <XCircle size={15} /> : <Info size={15} />}
          </span>
          <div><strong>{message.title}</strong>{message.detail && <span>{message.detail}</span>}</div>
          <button type="button" onClick={() => onDismiss(message.id)} aria-label="关闭通知"><X size={14} /></button>
        </div>
      ))}
    </div>
  )
}
