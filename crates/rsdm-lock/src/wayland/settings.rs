//! Keep filesystem writes off the Wayland event loop, including slow fsyncs.

use std::{sync::mpsc::{self, Receiver, TryRecvError}, thread};

use rsdm_core::domain::DesignConfig;

use super::App;

pub(super) struct SaveJob {
    result: Receiver<Result<DesignConfig, String>>,
}

impl App {
    pub(super) fn save_runtime_settings(&mut self) {
        let Some(settings) = self.menu.lock_settings() else { return; };
        self.menu.set_notice("Saving personal lock settings...", false);
        if self.settings_save.is_some() {
            return;
        }

        let mut design = self.ctx.design_config.clone();
        self.ctx.design.apply_runtime_to_config(&mut design);
        let config_path = self.ctx.config_path.clone();
        let (sender, result) = mpsc::channel();
        let worker = thread::Builder::new().name("rsdm-lock-settings".into()).spawn(move || {
            let saved = rsdm_infra::config::save_lock_runtime_settings(
                &config_path, &design, settings.size, settings.secondary_output,
            ).map(|()| design).map_err(|error| error.to_string());
            let _ = sender.send(saved);
        });
        match worker {
            Ok(_) => self.settings_save = Some(SaveJob { result }),
            Err(error) => {
                tracing::error!(%error, "could not start lock settings worker");
                self.menu.set_notice(format!("Save failed: {error}"), true);
            }
        }
    }

    pub(super) fn process_settings_save(&mut self) {
        let Some(job) = &self.settings_save else { return; };
        let result = match job.result.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("settings worker stopped".into()),
        };
        self.settings_save = None;
        match result {
            Ok(design) => {
                self.ctx.design_config = design;
                self.menu.set_notice("Personal lock settings saved", false);
            }
            Err(error) => {
                tracing::error!(%error, "could not save runtime lock settings");
                self.menu.set_notice(format!("Save failed: {error}"), true);
            }
        }
        self.needs_redraw = true;
    }
}
