# Maintainer: Benylin <lealmariop@gmail.com>
#
# Builds from $startdir (the pkgs/ CI checks out the tagged commit and runs makepkg in place).

pkgname=felt-tray
pkgver=0.7.1
pkgrel=1
pkgdesc="Felt on the Linux desktop: chat window, her voice for typed replies, alert pop-ups, screen reactions (KDE), tray icon / D-Bus API"
arch=('x86_64')
url="https://git.clockmost.com/pkgs/${pkgname}"
license=('LicenseRef-clockmost-private')
depends=(
  'gcc-libs' 'glibc'
  'webkit2gtk-4.1' 'gtk3'  # the chat window (wry)
  'xdg-utils'              # xdg-open: links out of the chat window
)
optdepends=(
  'pipewire: pw-play, the default player for her voice (or set `player` in the config)'
  'godot: runs her body (the felt-godot-overlay project), [body] in the config'
  'rhubarb-lip-sync: times her mouth to her voice (her body)'
)
makedepends=('rust' 'cargo' 'mold')
options=('!lto' '!debug')

prepare() {
  cd "$startdir"
  cargo fetch --locked --target "$(rustc -vV | sed -n 's/host: //p')"
}

build() {
  cd "$startdir"
  export RUSTFLAGS="-C link-arg=-fuse-ld=mold ${RUSTFLAGS:-}"
  export CARGO_TARGET_DIR=target
  cargo build --frozen --release
}

package() {
  cd "$startdir"
  install -Dm755 target/release/felt-tray "$pkgdir/usr/bin/felt-tray"
  install -Dm644 install/felt-tray.service "$pkgdir/usr/lib/systemd/user/felt-tray.service"
  install -Dm644 README.md "$pkgdir/usr/share/doc/$pkgname/README.md"
  install -Dm644 config.example.toml "$pkgdir/usr/share/doc/$pkgname/config.example.toml"
  install -Dm644 capture_blocklist.example.toml "$pkgdir/usr/share/doc/$pkgname/capture_blocklist.example.toml"
  # KDE: lets KWin grant /usr/bin/felt-tray per-window screenshots (screen reactions); inert elsewhere
  install -Dm644 install/org.clockmost.felt-tray.desktop "$pkgdir/usr/share/applications/org.clockmost.felt-tray.desktop"
  # her body's data (face poses, reactions, games, outfits): the [body] data_dir default
  install -Dm644 -t "$pkgdir/usr/share/$pkgname/body/" data/*
}
