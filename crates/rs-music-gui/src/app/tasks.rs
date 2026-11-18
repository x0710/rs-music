use eframe::egui;
use std::sync::mpsc;

pub(super) type Task<T> = mpsc::Receiver<Result<T, String>>;

pub(super) fn spawn<T: Send + 'static>(
    ctx: &egui::Context,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Task<T> {
    let (sender, receiver) = mpsc::channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = sender.send(work());
        ctx.request_repaint();
    });
    receiver
}

pub(super) fn finished<T>(task: &Task<T>) -> Option<Result<T, String>> {
    match task.try_recv() {
        Ok(result) => Some(result),
        Err(mpsc::TryRecvError::Empty) => None,
        Err(mpsc::TryRecvError::Disconnected) => Some(Err("Worker stopped".into())),
    }
}
