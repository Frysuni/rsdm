# Troubleshooting

## Lock screen cannot be unlocked

Switch to another VT (for example `Ctrl+Alt+F3`), log in, then run:

```sh
rsdm status
rsdm unlock
```

`rsdm unlock` resolves the current user before elevation and invokes itself via
sudo; if sudo is not installed it tries pkexec. When already root, use
`rsdm unlock --user alice` if more than one user has an active lock. The command
does not kill the locker: it validates its PID/UID/start time and asks that live
Wayland client to perform the protocol unlock. A completely crashed lock client
cannot be recovered by another client; the Wayland protocol intentionally keeps
an abandoned session locked, so restart the compositor/session in that case.

Use `rsdm logs --follow` for the combined system and user journal, or
`rsdm logs --component dm|idle|lock` to narrow it.

| symptom | cause / fix |
|---|---|
| Logs bleed onto the greeter screen | the unit lacks `StandardOutput=journal` / `StandardError=journal`. `rsdm dm` also refuses to start if stdout/stderr is the active TTY. |
| Compositor will not start / no GPU access | logind did not bind the session to the VT. Check the `rsdm` PAM service includes `pam_systemd` (automatic on NixOS) and that `[dm.tty].path`/`seat` are correct. |
| `rsdm lock` says "could not find wayland compositor" | it was not run inside a Wayland session (`$WAYLAND_DISPLAY` unset). Bind it to a compositor key. |
| Lock screen never unlocks | check `/etc/pam.d/rsdm-lock` (auth/account only). You can still switch VT and log in. |
| Keyring not unlocked | The keyring module must be in the `rsdm` login stack. NixOS: set `services.rsdm.keyring`. Arch: check the keyring lines in `/etc/pam.d/rsdm`. Confirm the keyring package is installed. See [keyrings.md](keyrings.md). |
| User services (panel/launcher) do not start | confirm they are `WantedBy=graphical-session.target` and enabled in `systemctl --user`; read `journalctl --user -b`. If the compositor manages its own session (niri/sway), rsdm does not touch its targets. |
| `another rsdm ... owns` | a second rsdm greeter holds the same VT; rsdm refuses to start twice on one terminal - fix `[dm.tty].path`. |
| `VT is owned by a live session; waiting` | a compositor or console login still holds the VT (typically a greeter restart during a running session). rsdm waits for that session to end instead of drawing over it; this is normal after `systemctl restart rsdm` or a rebuild. |
| `rsdm app` says `graphical-session.target is not active` | the app was launched outside a running graphical session (or the session manager never anchored the target). `rsdm app` units are stopped via that target, so launching without it would leak the app past logout. Check `systemctl --user status graphical-session.target`. |
| Greeter never appears at boot | the system did not reach `graphical.target`. Check `systemctl get-default`; the NixOS module sets `systemd.defaultUnit = "graphical.target"`. |
| Lock font or size differs from the Greeter | Remove an explicit `lock.size` override, use the same `dm.tty.path`, and start the updated DM before logging in so it can publish the console font. Without the snapshot Lock uses a fallback 8x16 font. Title art also needs matching design settings. |
| Wallpaper not visible behind the lock box | the wallpaper shows through only when `[lock.design].wallpaper` is set; check the path and that `wallpaper_dim` is not `0` (which blacks it out). |
| Lock UI appears on the wrong monitor | set `[lock].primary_output` to the connector name shown by `niri msg outputs` (or the compositor equivalent). Without it rsdm selects the largest current pixel mode. |
| Lock looks blurry at scale 1.5 | use a compositor with `fractional-scale-v1` and `viewporter` support and check debug logs for the advertised `scale_120`; rsdm otherwise uses a crisp integer-scale fallback. |
| TTY greeter is cloned or small on a larger monitor | fbcon exposes one global VT grid, not per-output surfaces. Disable the unwanted connector for the console with a kernel `video=<connector>:d` parameter; see [display-manager.md](display-manager.md#multiple-monitors). |

## Useful commands

```sh
rsdm validate-config --config /etc/rsdm.toml   # check config, print warnings
journalctl -u rsdm.service -b                          # greeter logs
journalctl --user -b                                   # session/user-service logs
```

Raise verbosity with `[logging] level = "debug"` (or `trace`), or temporarily
`RUST_LOG=rsdm=debug`.
