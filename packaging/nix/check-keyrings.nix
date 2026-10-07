{
  self,
  nixpkgs,
  system,
}:
let
  check = case:
    let
      cfg = (nixpkgs.lib.nixosSystem {
        inherit system;
        modules = [
          self.nixosModules.default
          {
            system.stateVersion = "26.05";
            services.rsdm = {
              enable = true;
              keyring = case.choice;
            };
            security.pam.services.example-gnome.enableGnomeKeyring = case.gnomePam or false;
            security.pam.services.example-plasma.kwallet.enable = case.kwalletPam or false;
            services.gnome.gnome-keyring.enable = case.gnomeService or false;
            security.pam.services.rsdm-lock.enableGnomeKeyring = case.lockGnome or false;
          }
        ];
      }).config;
      pam = cfg.security.pam.services.rsdm;
    in
    assert pam.enableGnomeKeyring == case.expectGnome;
    assert pam.kwallet.enable == case.expectKwallet;
    true;
  cases = [
    {
      choice = "auto";
      expectGnome = false;
      expectKwallet = false;
    }
    {
      choice = "auto";
      gnomePam = true;
      expectGnome = true;
      expectKwallet = false;
    }
    {
      choice = "auto";
      kwalletPam = true;
      expectGnome = false;
      expectKwallet = true;
    }
    {
      choice = "auto";
      gnomeService = true;
      expectGnome = true;
      expectKwallet = false;
    }
    {
      choice = "auto";
      gnomePam = true;
      kwalletPam = true;
      expectGnome = true;
      expectKwallet = false;
    }
    {
      choice = "auto";
      gnomeService = true;
      kwalletPam = true;
      expectGnome = true;
      expectKwallet = false;
    }
    {
      choice = "gnome";
      kwalletPam = true;
      expectGnome = true;
      expectKwallet = false;
    }
    {
      choice = "kwallet";
      gnomePam = true;
      kwalletPam = true;
      expectGnome = false;
      expectKwallet = true;
    }
    {
      choice = "none";
      gnomePam = true;
      kwalletPam = true;
      expectGnome = false;
      expectKwallet = false;
    }
    {
      choice = "auto";
      lockGnome = true;
      kwalletPam = true;
      expectGnome = false;
      expectKwallet = true;
    }
  ];
in
builtins.all check cases
