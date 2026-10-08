# Lock screen

`rsdm lock` is an `ext-session-lock-v1` Wayland client. It runs inside an already
running Wayland session, covers every output, and unlocks only when the seated
user types their password. It renders the same design as the greeter, drawn onto
a software framebuffer.

```sh
rsdm lock --config /etc/rsdm.toml
```

It must run inside your session (it needs `$WAYLAND_DISPLAY`), so bind it to a
key in your compositor or enable rsdm's user-session idle service.

## Binding it

```ini
# Hyprland (~/.config/hypr/hyprland.conf)
bind = SUPER, L, exec, rsdm lock
```

```kdl
// niri (~/.config/niri/config.kdl)
binds { Super+L { spawn "rsdm" "lock"; } }
```

Built-in idle locking:

```toml
[idle]
enable = true
timeout = 300
on_lock = []
on_unlock = []
```

See [idle.md](idle.md). External idle daemons remain supported:

```sh
# sway/wlroots
swayidle -w timeout 300 'rsdm lock' before-sleep 'rsdm lock'
# Hyprland
hypridle   # with a listener that runs `rsdm lock`
```

See [compositors/niri.md](compositors/niri.md) and
[compositors/hyprland.md](compositors/hyprland.md).

Set `[lock].enable = true` to allow locking; with it `false`, `rsdm lock`
refuses to lock (so an idle daemon can be wired up without ever blanking).

## Multiple outputs and scaling

The unlock UI is rendered on exactly one output. Other outputs remain securely
covered but contain no clock, form, status, menu, or keyboard hints:

```toml
[lock]
enable = true
primary_output = "DP-1"       # omit to select the largest connected output
secondary_output = "background" # background | black | off
# size = 2                       # 1..12; omit for TTY-like automatic sizing
```

`background` keeps only `[lock.design]` wallpaper/effects. `black` paints opaque
black. `off` uses a compositor output-power protocol when available and
otherwise falls back to opaque black without changing output topology.
Find niri names with `niri msg outputs`.

Wallpaper scaling is cached separately for each output at its physical buffer
size. Resizing or changing output scale replaces that output's cache; animation,
text size, and dim changes reuse the prepared image. If cache allocation fails,
Lock still renders by resampling the original image.

Each output reuses its software canvas and at most two SHM buffers. Buffers held
by the compositor remain read-only until `wl_buffer.release`, including during
resize; when both are busy, the latest redraw stays pending. Unchanged black
secondary frames are skipped. Rendering still copies each submitted software
frame into SHM; it does not introduce borrowed pixel storage or GPU rendering.
With a static background, the visible clock requests a redraw when its displayed
minute/date changes, without waiting for keyboard or compositor events.

Sleep and Hibernate shortcuts are offered only when logind's `CanSuspend` or
`CanHibernate` reports `yes` or `challenge` for the locker user. The snapshot is
queried in a background worker with a five-second deadline covering connection
setup and both calls; until it succeeds, these shortcuts are hidden. The actual
request still goes through logind with interactive authorization enabled, so a
policy change or unavailable authorization agent can reject the action without
unlocking the screen.

The locker renders a separate physical-pixel buffer for every output. On
fractionally scaled outputs (for example niri `scale 1.5`) it follows
`fractional-scale-v1` and `viewporter`, preserving the TTY-like density and crisp
pixel glyphs instead of asking the compositor to enlarge a logical-size buffer.

Automatic sizing targets roughly 68 text rows, comparable to a typical TTY
(`size = 2` on a 1080p physical buffer). Set `[lock].size` from `1` through `12`
to force an exact integer bitmap zoom. Both size and the secondary-output policy
can be previewed immediately through `F1`.

## Look

The locker design is `[lock.design]` - the same fields as the greeter, set
independently. A field left unset uses its own default, not the greeter's value.
`F1` opens the runtime switcher (when `menu = true`). Besides the shared design
controls, the lock screen has Lock size, Secondary outputs, and Save settings.

## Wallpaper, dim, and opacity

`[lock.design]` adds three locker-only fields on top of the shared design (the
greeter has no wallpaper and ignores them):

| key                  | effect                                                            |
|----------------------|-------------------------------------------------------------------|
| `wallpaper`          | a PNG/JPEG drawn as the base layer (cover-fit, center-cropped)    |
| `wallpaper_dim`      | how dark the wallpaper is drawn: `0` blacks it out, `10` leaves it untouched |
| `background_opacity` | opacity of the animated background where it draws over the wallpaper, `0` (wallpaper only) .. `10` (opaque) |

Layering, from back to front:

1. wallpaper (or the solid theme background if none), darkened by `wallpaper_dim`,
2. the animated `background` (matrix, fire, ...) at `background_opacity`,
3. the login box and status/footer.

On a secondary output in `background` mode, layer 3 is intentionally omitted.

How the box treats the layer behind it depends on the background:

- **Full-field backgrounds** (`plasma`, `fire`) own the whole field, so they
  show through everything - the box frame, the banner, and the text. Only the
  glyph shapes themselves are opaque; the field shows through their cells. There
  is no black fill anywhere.
- **Sparse backgrounds** (`matrix`, `rain`, `starfield`) are dots over the theme
  background, so the box matches that background behind its text rather than
  punching a hole.
- **With a wallpaper and no animated background**, the box is transparent and the
  wallpaper shows through directly.

`wallpaper_dim` and `background_opacity` are also adjustable at runtime via `F1`
(Wallpaper dim, Background opacity) when a wallpaper is configured. Select Save
settings to persist every lock-menu change into a writable TOML file while
preserving its comments. `dim` is accepted as an alias for `wallpaper_dim`.

NixOS generates `/etc/rsdm.toml` as an immutable `/nix/store` target, so
runtime saving is refused with an explanation instead of attempting a partial or
privileged write. Persist the same values under `services.rsdm.lock` and rebuild.

## Authentication

The password is verified through the `[lock].pam_service` PAM stack (default
`rsdm-lock`), which runs only `auth`/`account` - it never opens a session or
changes tokens. Only the current user can unlock: the username comes from the
process owner, not from input. Failed attempts are rate limited. If PAM is
misconfigured you can still switch to another VT and log in. From that VT,
`rsdm unlock` uses sudo (or pkexec when sudo is unavailable) to request a
verified emergency unlock from the live locker. This deliberately bypasses PAM
and therefore always requires root authorization.

Enter starts PAM authentication even with an empty password field, allowing
configured fingerprint or token modules to run. PAM decides whether the attempt
succeeds, and failed attempts remain rate limited.

## Preview without a compositor

Render the locker look to a PNG (handy for tuning themes/wallpaper):

```sh
cargo run -p rsdm-lock --example preview -- \
  /tmp/lock.png <theme> "" <border> <background> <frame> [wallpaper|none] [bg_opacity]
xdg-open /tmp/lock.png
```
