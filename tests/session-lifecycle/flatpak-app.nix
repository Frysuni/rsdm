{ pkgs }:

let
  application = import ./window-app.nix { inherit pkgs; };
  arch = pkgs.stdenv.hostPlatform.parsed.cpu.name;
  runtimeMetadata = pkgs.writeText "test-flatpak-runtime" ''
    [Runtime]
    name=org.rsdm.TestRuntime
    runtime=org.rsdm.TestRuntime/${arch}/test
    sdk=org.rsdm.TestRuntime/${arch}/test
  '';
  appMetadata = pkgs.writeText "test-flatpak-app" ''
    [Application]
    name=org.rsdm.WindowTest
    runtime=org.rsdm.TestRuntime/${arch}/test
    command=test-window
    [Context]
    shared=ipc;
    sockets=wayland;
    filesystems=/nix:ro;
    [Environment]
    GSK_RENDERER=cairo
  '';
in pkgs.writeShellScript "install-test-flatpak" ''
  set -eu
  directory="$HOME/flatpak-fixture"
  mkdir -p "$directory/runtime/usr/bin" "$directory/runtime/files" "$directory/app/files/bin" "$directory/app/export"
  cp ${runtimeMetadata} "$directory/runtime/metadata"
  cp ${appMetadata} "$directory/app/metadata"
  cp ${application} "$directory/app/files/bin/test-window"
  flatpak build-export --runtime "$directory/repo" "$directory/runtime" test
  flatpak build-export "$directory/repo" "$directory/app" test
  flatpak remote-add --user --no-gpg-verify test "$directory/repo"
  flatpak install --user --noninteractive test org.rsdm.TestRuntime org.rsdm.WindowTest
''
