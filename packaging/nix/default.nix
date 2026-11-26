{
  lib,
  stdenv,
  rustPlatform,
  pkg-config,
  buildFHSEnv,

  dbus,
  udev,
  openssl,
  wayland,
  libxkbcommon,
  libx11,
  libxcursor,
  libxi,
  libxrandr,
}:
let
  cargoToml = fromTOML (builtins.readFile ../../Cargo.toml);
  version = cargoToml.workspace.package.version;

  aurora-unwrapped = rustPlatform.buildRustPackage {
    pname = "aurora";
    inherit version;
    src = lib.cleanSourceWith {
      src = ./../..;
      filter =
        path: _type:
        let
          name = baseNameOf (toString path);
        in
        !(builtins.elem name [
          ".git"
          ".jj"
          "target"
          "result"
        ]);
    };
    cargoLock.lockFile = ../../Cargo.lock;

    nativeBuildInputs = [
      pkg-config
    ];

    buildInputs = [
      dbus
      udev
      openssl
      wayland
      libxkbcommon
      libx11
      libxcursor
      libxi
      libxrandr
    ];

    cargoBuildFlags = [
      "--locked"
      "-p"
      "aurora"
      "-p"
      "steamhost"
      "-p"
      "vk_overlay"
      "-p"
      "html_shim"
    ];

    doCheck = false;

    installPhase = ''
      runHook preInstall

      releaseDir=target/${stdenv.targetPlatform.rust.cargoShortTarget}/release

      install -d $out/libexec/aurora $out/bin
      install -m755 $releaseDir/aurora $out/libexec/aurora/aurora
      install -m755 $releaseDir/steamhost $out/libexec/aurora/steamhost
      install -m755 $releaseDir/libVkLayer_aurora_overlay.so $out/libexec/aurora/
      install -m755 $releaseDir/libaurora_htmlsurface.so $out/libexec/aurora/
      ln -s ../libexec/aurora/aurora $out/bin/aurora

      install -Dm644 LICENSE $out/share/licenses/aurora/LICENSE
      install -Dm644 README.md $out/share/doc/aurora/README.md

      install -Dm644 packaging/nix/aurora.desktop $out/share/applications/aurora.desktop
      install -Dm644 assets/icon-256.png $out/share/icons/hicolor/256x256/apps/aurora.png
      runHook postInstall
    '';

    meta = {
      description = "Lightweight native Steam desktop client for Linux";
      homepage = "https://auroraclient.eu/";
      license = lib.licenses.mit;
      mainProgram = "aurora";
      platforms = lib.platforms.linux;
    };
  };
in
buildFHSEnv {
  pname = "aurora";
  inherit version;
  executableName = "aurora";

  runScript = "${aurora-unwrapped}/libexec/aurora/aurora";

  multiArch = true;
  includeClosures = true;

  targetPkgs =
    pkgs:
    with pkgs;
    [
      aurora-unwrapped
      bash
      coreutils
      file
      lsb-release
      pciutils
      usbutils
      xdg-utils
      xz
      zenity

      dbus
      openssl
      udev
      wayland
      libxkbcommon
      libx11
      libxcursor
      libxi
      libxrandr
      libGL
      vulkan-loader

      # Optional features: native Adwaita styling and store trailers.
      gtk4
      libadwaita
      ffmpeg
      libpulseaudio
    ]
    ++ lib.optionals stdenv.hostPlatform.isx86_64 [
      glibc_multi.bin
    ];

  multiPkgs =
    pkgs: with pkgs; [
      glibc
      libxcrypt
      libGL
      libdrm
      libgbm
      udev
      libudev0-shim
      libva
      vulkan-loader
      networkmanager
      libcap
    ];

  profile = ''
    unset GIO_EXTRA_MODULES

    export SDL_JOYSTICK_DISABLE_UDEV=1

    export GTK_IM_MODULE='xim'

    export LIBGL_DRIVERS_PATH=/run/opengl-driver/lib/dri:/run/opengl-driver-32/lib/dri
    export __EGL_VENDOR_LIBRARY_DIRS=/run/opengl-driver/share/glvnd/egl_vendor.d:/run/opengl-driver-32/share/glvnd/egl_vendor.d
    export LIBVA_DRIVERS_PATH=/run/opengl-driver/lib/dri:/run/opengl-driver-32/lib/dri
    export VDPAU_DRIVER_PATH=/run/opengl-driver/lib/vdpau:/run/opengl-driver-32/lib/vdpau

    # pressure-vessel expects the timezone data under /usr/share.
    tz="$(readlink -f /etc/localtime | grep -P -o '(?<=/zoneinfo/).*$')"
    ln -sf /usr/share/zoneinfo/$tz /etc/localtime
  '';

  # Steam expects /sbin/ldconfig to exist; a symlink loops inside nested
  # containers, so copy it.
  extraBuildCommands = ''
    cp -f $out/usr/{bin,sbin}/ldconfig
  '';

  extraBwrapArgs = [
    "--bind /etc/zoneinfo /usr/share/zoneinfo"
    "--bind-try /tmp/dumps /tmp/dumps"
  ];

  # buildFHSEnv only puts bin/ in its output. Expose the desktop file, icon,
  # docs and license installed by the unwrapped package as well.
  extraInstallCommands = ''
    ln -s ${aurora-unwrapped}/share $out/share
  '';

  passthru = {
    inherit aurora-unwrapped;
    unwrapped = aurora-unwrapped;
  };

  meta = {
    description = "Lightweight native Steam desktop client for Linux";
    homepage = "https://auroraclient.eu/";
    license = lib.licenses.mit;
    mainProgram = "aurora";
    platforms = lib.platforms.linux;
  };
}
