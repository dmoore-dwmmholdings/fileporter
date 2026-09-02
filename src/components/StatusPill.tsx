import type { BatchState } from '../types/view-models';

export function StatusPill({ state }: { state: BatchState }) {
  const text: Record<BatchState, string> = {
    queued: 'Queued', waiting: 'Waiting for device', preparing: 'Preparing', sending: 'Sending', receiving: 'Receiving',
    verifying: 'Verifying', complete: 'Complete', partial: 'Partially complete', paused: 'Paused', cancelled: 'Cancelled', failed: 'Failed'
  };
  return <span className={`status-pill status-${state}`}>{text[state]}</span>;
}
