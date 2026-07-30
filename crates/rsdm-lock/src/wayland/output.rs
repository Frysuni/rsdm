use std::process::Command;

use rsdm_core::domain::SecondaryOutput;
use smithay_client_toolkit::reexports::client::{Proxy, protocol::wl_output};

use super::App;

impl App {
    pub(super) fn choose_primary_output(&mut self) {
        let outputs: Vec<_> = self.output_state.outputs().collect();
        if outputs.is_empty() {
            self.primary_output = None;
            return;
        }

        let configured = self
            .ctx
            .primary_output
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty());
        let explicit = configured.and_then(|wanted| {
            outputs
                .iter()
                .find(|output| self.output_matches(output, wanted))
                .cloned()
        });
        if configured.is_some() && explicit.is_none() {
            tracing::warn!(
                output = configured.unwrap_or_default(),
                "configured primary lock output is unavailable"
            );
        }

        let selected = explicit.or_else(|| {
            outputs.into_iter().max_by(|left, right| {
                self.output_pixel_area(left)
                    .cmp(&self.output_pixel_area(right))
                    .then_with(|| self.output_name(right).cmp(&self.output_name(left)))
            })
        });
        if self.primary_output == selected {
            return;
        }

        let name = selected
            .as_ref()
            .map(|output| self.output_name(output))
            .unwrap_or_else(|| "<none>".to_string());
        tracing::info!(output = %name, "selected primary lock output");
        self.primary_output = selected;
    }

    pub(super) fn output_name(&self, output: &wl_output::WlOutput) -> String {
        self.output_state
            .info(output)
            .and_then(|info| info.name)
            .unwrap_or_else(|| format!("wl-output-{}", output.id().protocol_id()))
    }

    pub(super) fn output_scale_120(&self, output: &wl_output::WlOutput) -> u32 {
        self.output_state
            .info(output)
            .map(|info| info.scale_factor.max(1) as u32 * 120)
            .unwrap_or(120)
    }

    pub(super) fn power_off_secondary_outputs(&mut self) {
        if self.ctx.secondary_output != SecondaryOutput::Off {
            return;
        }
        let Some(primary) = self.primary_output.clone() else {
            return;
        };

        let outputs: Vec<_> = self.output_state.outputs().collect();
        for output in outputs {
            if output == primary {
                continue;
            }
            let name = self.output_name(&output);
            if self.powered_off_outputs.contains(&name) {
                continue;
            }
            if niri_output_command(&name, "off") {
                self.powered_off_outputs.push(name);
            } else if !self.off_fallback_warned {
                tracing::warn!(
                    "secondary_output=off is unavailable; painting secondary outputs black"
                );
                self.off_fallback_warned = true;
            }
        }
    }

    pub(super) fn restore_secondary_outputs(&mut self) {
        for output in std::mem::take(&mut self.powered_off_outputs) {
            if !niri_output_command(&output, "on") {
                tracing::error!(%output, "failed to restore secondary output");
                self.powered_off_outputs.push(output);
            }
        }
    }

    pub(super) fn apply_secondary_output_policy(&mut self, policy: SecondaryOutput) {
        self.ctx.secondary_output = policy;
        if policy == SecondaryOutput::Off {
            self.power_off_secondary_outputs();
        } else {
            self.restore_secondary_outputs();
        }
        self.needs_redraw = true;
    }

    fn output_matches(&self, output: &wl_output::WlOutput, wanted: &str) -> bool {
        self.output_state.info(output).is_some_and(|info| {
            info.name
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(wanted))
                || info
                    .description
                    .as_deref()
                    .is_some_and(|description| description.eq_ignore_ascii_case(wanted))
                || format!("{} {}", info.make, info.model).eq_ignore_ascii_case(wanted)
        })
    }

    fn output_pixel_area(&self, output: &wl_output::WlOutput) -> u64 {
        let Some(info) = self.output_state.info(output) else {
            return 0;
        };
        if let Some(mode) = info.modes.iter().find(|mode| mode.current) {
            return positive_area(mode.dimensions);
        }
        let scale = info.scale_factor.max(1) as u64;
        info.logical_size
            .map(positive_area)
            .unwrap_or_default()
            .saturating_mul(scale.saturating_mul(scale))
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.restore_secondary_outputs();
    }
}

fn positive_area((width, height): (i32, i32)) -> u64 {
    (width.max(0) as u64).saturating_mul(height.max(0) as u64)
}

fn niri_output_command(output: &str, action: &str) -> bool {
    if std::env::var_os("NIRI_SOCKET").is_none() {
        return false;
    }
    match Command::new("niri")
        .args(["msg", "output", output, action])
        .status()
    {
        Ok(status) if status.success() => true,
        Ok(status) => {
            tracing::warn!(%status, %output, %action, "niri output command failed");
            false
        }
        Err(error) => {
            tracing::warn!(%error, %output, %action, "could not run niri output command");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_area_rejects_negative_protocol_values() {
        assert_eq!(positive_area((-1, 1080)), 0);
        assert_eq!(positive_area((3840, 2160)), 8_294_400);
    }
}
