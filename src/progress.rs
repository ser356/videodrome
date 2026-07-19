use std::cell::RefCell;
use std::time::Duration;

use indicatif::{ProgressBar, ProgressStyle};

const TICK_STRINGS: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Reporte de progreso desacoplado del renderizado: la lógica de negocio en
/// `recommend.rs` reporta etapas/avances sin saber si el consumidor es la
/// CLI (barras indicatif) o la TUI (eventos por canal).
pub trait Progress {
    fn stage(&self, msg: &str, total: u64);
    fn inc(&self);
    fn finish(&self);
}

pub struct CliProgress {
    bar: RefCell<Option<ProgressBar>>,
}

impl CliProgress {
    pub fn new() -> Self {
        Self {
            bar: RefCell::new(None),
        }
    }
}

impl Progress for CliProgress {
    fn stage(&self, msg: &str, total: u64) {
        if let Some(pb) = self.bar.borrow_mut().take() {
            pb.finish_and_clear();
        }
        let pb = if total == 0 {
            let pb = ProgressBar::new_spinner();
            pb.set_style(
                ProgressStyle::with_template("{spinner:.cyan}  {msg}")
                    .expect("indicatif template is a hardcoded literal — always valid")
                    .tick_strings(TICK_STRINGS),
            );
            pb
        } else {
            let pb = ProgressBar::new(total);
            pb.set_style(
                ProgressStyle::with_template(
                    "{spinner:.cyan}  {msg}  {bar:28.cyan/white.dim}  {pos}/{len}",
                )
                .expect("indicatif template is a hardcoded literal — always valid")
                .tick_strings(TICK_STRINGS),
            );
            pb
        };
        pb.set_message(msg.to_string());
        pb.enable_steady_tick(Duration::from_millis(80));
        *self.bar.borrow_mut() = Some(pb);
    }

    fn inc(&self) {
        if let Some(pb) = self.bar.borrow().as_ref() {
            pb.inc(1);
        }
    }

    fn finish(&self) {
        if let Some(pb) = self.bar.borrow_mut().take() {
            pb.finish_and_clear();
        }
    }
}
