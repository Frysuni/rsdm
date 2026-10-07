# Keyrings

rsdm does not implement, talk to, or know about any keyring (gnome-keyring,
kwallet, a self-written agent, ...). It unlocks the keyring the same way
`login(1)`, gdm and sddm do: it is the login, so the keyring module runs inside
the very PAM login that checks your password, and unlocks the keyring with that
same password. rsdm just runs the PAM stack and carries the result into your
session.

## How it works

When you log in, rsdm runs one PAM stack (the service named `rsdm`) and lets the
system do everything in it:

1. PAM checks your password. A keyring `auth` module in the stack quietly
   remembers the password at this point.
2. rsdm opens the PAM session. `pam_systemd` registers the logind session (so the
   compositor can drive the screen), and the keyring `session` module unlocks the
   keyring with the remembered password and starts its agent.
3. The keyring module starts the agent as your user (it drops privileges itself)
   and writes its address into the PAM environment (`SSH_AUTH_SOCK`,
   `GNOME_KEYRING_CONTROL`, ...).
4. rsdm reads that environment back from PAM and hands it to your session, so
   every app you launch finds the already-unlocked keyring.
5. When the session ends, rsdm closes the PAM session and the keyring relocks.

That is the whole thing. rsdm never sees, copies or forwards your password for
keyring purposes - it is already in the right place because the keyring runs in
the login itself. There is no second password prompt, no separate service, no
helper process.

## Compared to other display managers

- `login(1)` / agetty, gdm, sddm, lightdm: one PAM login stack with the keyring
  module in it. rsdm does exactly this.
- greetd and other minimal greeters: authenticate in one place and start the
  session elsewhere, so the keyring does not unlock out of the box; the usual fix
  is to fold a login stack back in (`auth substack login`). rsdm avoids that
  split - its greeter is the login, so the keyring is in the same stack.

So rsdm is deliberately unremarkable here: if you know how `login` unlocks a
keyring, you know how rsdm does.

## Why you still name a keyring daemon

A keyring is unlocked by its PAM module (`pam_gnome_keyring.so`,
`pam_kwallet5.so`, your own `.so`). PAM does not auto-detect which one you use, so
the module has to be named in the `rsdm` stack. That is true for every login on
Linux, not an rsdm quirk - `/etc/pam.d/login` names it too.

### NixOS

Set one option:

```nix
services.rsdm.keyring = "auto";  # the default
```

- `auto` selects one detected keyring: GNOME Keyring if another PAM service
  enables it or `services.gnome.gnome-keyring.enable` is set; otherwise KWallet
  if another PAM service enables it. If both are detected, GNOME Keyring takes
  precedence. Set `keyring = "kwallet"` to choose KWallet on a mixed system.
- `gnome` / `kwallet` force one.
- `none` runs no keyring module.

The option simply inserts the right keyring module into the `rsdm` PAM stack, in
the correct order (the keyring `auth` module must come before the
`sufficient pam_unix` line, which the structured option handles for you).

For a custom keyring on NixOS, set `keyring = "none"` and add your module to the
`rsdm` stack yourself:

```nix
security.pam.services.rsdm.rules.auth.my_keyring = {
  control = "optional"; modulePath = "/path/to/pam_mykeyring.so"; order = ...;
};
# and the matching session rule with your auto-start argument
```

### Arch and other imperative distros

rsdm ships `/etc/pam.d/rsdm`. It already pulls in your normal login stack
(`include system-login`) and adds the two common keyrings as `optional` lines, so
gnome-keyring and kwallet work as soon as the module is installed and ignore
themselves when it is not:

```
auth      include   system-login
auth      optional  pam_gnome_keyring.so
account   include   system-login
password  include   system-login
password  optional  pam_gnome_keyring.so
session   include   system-login
session   optional  pam_gnome_keyring.so auto_start
```

To use a different keyring, replace the `pam_gnome_keyring.so` / `pam_kwallet5.so`
lines with your module (`auth` to capture the password, `session ... auto_start`
to unlock). To disable keyring unlock, delete those lines. Mirror whatever your
`/etc/pam.d/login` does - the rules are identical.

## Custom / self-written keyrings

rsdm is fully agnostic. Any keyring works with zero rsdm changes as long as it
ships a PAM module that follows the normal contract:

- an `auth` step that remembers the login password,
- a `session` step that unlocks the keyring and starts the agent as the user,
- and (if apps need to find the agent) writes its address into the PAM
  environment, which rsdm forwards verbatim.

Add that module to the `rsdm` stack and you are done. A keyring with no PAM
module cannot be unlocked at login by rsdm - or by `login`, gdm or sddm either;
that is a property of PAM, not of rsdm.

## Troubleshooting

- Keyring not unlocked: confirm the keyring's PAM module is actually in the
  `rsdm` stack (NixOS: `services.rsdm.keyring`; Arch: the lines in
  `/etc/pam.d/rsdm`) and that the keyring package is installed.
- It unlocks but apps cannot reach it: check that the agent variables
  (`SSH_AUTH_SOCK`, ...) are present in the session - rsdm logs which keyring
  variables PAM published right after login.
