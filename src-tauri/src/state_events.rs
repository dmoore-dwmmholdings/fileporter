//! Runtime-neutral state-change signalling. Platform adapters consume these
//! events; core persistence never depends on a UI or notification API.
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::{mpsc, Notify};
#[cfg(not(feature = "desktop"))]
use tokio::task::JoinHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateEventKind {
    Progress,
    Terminal,
}

pub trait StateEventSink: Send + Sync {
    fn emit(&self, kind: StateEventKind);
}
#[derive(Clone)]
pub struct StateEvents(Arc<dyn StateEventSink>);
impl StateEvents {
    pub fn noop() -> Self {
        Self(Arc::new(Noop))
    }
    pub fn emit(&self, kind: StateEventKind) {
        self.0.emit(kind)
    }
}
struct Noop;
impl StateEventSink for Noop {
    fn emit(&self, _: StateEventKind) {}
}

#[cfg_attr(not(feature = "desktop"), allow(dead_code))] // The production desktop reactor owns this queue; no-default tests invoke it directly.
struct Core {
    progress: AtomicBool,
    terminals: Mutex<VecDeque<StateEventKind>>,
    wake: Notify,
    closed: AtomicBool,
}
#[derive(Clone)]
#[cfg_attr(not(feature = "desktop"), allow(dead_code))] // See Core.
struct Coalescing(Arc<Core>);
impl StateEventSink for Coalescing {
    fn emit(&self, kind: StateEventKind) {
        if self.0.closed.load(Ordering::Acquire) {
            return;
        }
        match kind {
            StateEventKind::Progress => {
                self.0.progress.store(true, Ordering::Release);
            }
            StateEventKind::Terminal => self
                .0
                .terminals
                .lock()
                .expect("event queue poisoned")
                .push_back(kind),
        }
        self.0.wake.notify_one();
    }
}
#[cfg_attr(not(feature = "desktop"), allow(dead_code))] // Constructed by the desktop reactor; test builds cover delivery semantics.
pub struct StateEventWorker {
    core: Arc<Core>,
    #[cfg(feature = "desktop")]
    task: tauri::async_runtime::JoinHandle<()>,
    #[cfg(not(feature = "desktop"))]
    task: JoinHandle<()>,
}
impl StateEventWorker {
    #[cfg_attr(not(feature = "desktop"), allow(dead_code))] // Called by desktop setup and isolated core tests.
    pub fn bounded(output_capacity: usize) -> (StateEvents, mpsc::Receiver<StateEventKind>, Self) {
        let core = Arc::new(Core {
            progress: AtomicBool::new(false),
            terminals: Mutex::new(VecDeque::new()),
            wake: Notify::new(),
            closed: AtomicBool::new(false),
        });
        let (tx, rx) = mpsc::channel(output_capacity.max(1));
        let worker_core = core.clone();
        // Desktop setup runs on Tauri's synchronous thread. Use Tauri's
        // runtime entry point so this worker can also be constructed there.
        #[cfg(feature = "desktop")]
        let task = tauri::async_runtime::spawn(async move {
            worker_loop(worker_core, tx).await;
        });
        #[cfg(not(feature = "desktop"))]
        let task = tokio::spawn(async move {
            worker_loop(worker_core, tx).await;
        });
        (
            StateEvents(Arc::new(Coalescing(core.clone()))),
            rx,
            Self { core, task },
        )
    }
    #[cfg_attr(not(feature = "desktop"), allow(dead_code))] // Called during desktop shutdown and isolated core tests.
    pub async fn shutdown(self) {
        self.core.closed.store(true, Ordering::Release);
        self.core.wake.notify_one();
        let _ = self.task.await;
    }
}

async fn worker_loop(worker_core: Arc<Core>, tx: mpsc::Sender<StateEventKind>) {
    loop {
        worker_core.wake.notified().await;
        loop {
            let next = worker_core
                .terminals
                .lock()
                .expect("event queue poisoned")
                .pop_front();
            if let Some(event) = next {
                let _ = tx.send(event).await;
            } else {
                break;
            }
        }
        if worker_core.progress.swap(false, Ordering::AcqRel) {
            let _ = tx.send(StateEventKind::Progress).await;
        }
        if worker_core.closed.load(Ordering::Acquire) {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn progress_coalesces_terminals_do_not() {
        let (sink, mut rx, worker) = StateEventWorker::bounded(8);
        sink.emit(StateEventKind::Progress);
        sink.emit(StateEventKind::Progress);
        sink.emit(StateEventKind::Terminal);
        sink.emit(StateEventKind::Terminal);
        assert_eq!(rx.recv().await, Some(StateEventKind::Terminal));
        assert_eq!(rx.recv().await, Some(StateEventKind::Terminal));
        assert_eq!(rx.recv().await, Some(StateEventKind::Progress));
        worker.shutdown().await;
    }
    #[tokio::test]
    async fn shutdown_drains_already_queued_terminal() {
        let (sink, mut rx, worker) = StateEventWorker::bounded(2);
        sink.emit(StateEventKind::Terminal);
        worker.shutdown().await;
        assert_eq!(rx.recv().await, Some(StateEventKind::Terminal));
    }
}
