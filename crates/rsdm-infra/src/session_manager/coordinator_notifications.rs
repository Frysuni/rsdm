//! Protocol cancellation and authoritative logind shutdown changes.

use std::sync::atomic::Ordering;

use rsdm_core::domain::SessionPhase;

use super::{SessionError, coordinator::Coordinator, processes::monotonic_usec};

impl Coordinator {
    pub fn notifications(&mut self) -> Result<(), SessionError> {
        #[cfg(feature = "xsmp")]
        while let Some(notice) = self.xsmp.poll() {
            if matches!(notice, super::xsmp::Notice::Cancelled | super::xsmp::Notice::Failed(_)) {
                if let super::xsmp::Notice::Failed(reason) = notice {
                    tracing::warn!(%reason, "XSMP save failed");
                }
                if let Some(control) = &self.shutdown {
                    if !control.noncancelable.load(Ordering::SeqCst) {
                        control.cancelled.store(true, Ordering::SeqCst);
                    }
                }
            }
        }
        while let Ok(notice) = self.notices.try_recv() {
            if notice.preparing {
                let proposed = monotonic_usec()?.saturating_add(notice.budget_usec);
                self.begin_stop("external-shutdown", Some(proposed))?;
                let control = self.shutdown.as_ref().expect("shutdown control");
                let old = control.hard_deadline.load(Ordering::SeqCst);
                let deadline = if old == 0 { proposed } else { old.min(proposed) };
                control.force(deadline);
                self.record.shutdown_deadline_usec = Some(deadline);
                self.save()?;
            } else {
                self.system_shutdown_cancelled()?;
            }
        }
        Ok(())
    }

    fn system_shutdown_cancelled(&mut self) -> Result<(), SessionError> {
        if self.lifecycle.phase != SessionPhase::Preparing || self.action != "external-shutdown" {
            return Ok(());
        }
        let Some(control) = &self.shutdown else { return Ok(()); };
        // A caller cannot cancel an external shutdown. Logind itself can revoke
        // it while application preparation still precedes infrastructure stop.
        control.noncancelable.store(false, Ordering::SeqCst);
        control.cancelled.store(true, Ordering::SeqCst);
        control.hard_deadline.store(0, Ordering::SeqCst);
        self.record.shutdown_deadline_usec = None;
        self.xsmp.cancel();
        self.save()
    }
}
