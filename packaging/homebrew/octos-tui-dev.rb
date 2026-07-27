# Prerelease-channel formula for octos-tui (class OctosTuiDev).
#
# This is the DEV/prerelease sibling of Formula/octos-tui.rb. It is rendered by
# .github/workflows/publish-homebrew.yml on a PRERELEASE tag push (a tag with a
# '-', e.g. v0.2.2-rc.15) into Formula/octos-tui-dev.rb, filling the same
# __VERSION__/__TAG__/__SHA_*__ placeholders from that prerelease's assets. The
# stable Formula/octos-tui.rb is NEVER touched by a prerelease tag, so
# `brew install octos-org/octos-tui/octos-tui` stays on the latest STABLE while
# `brew install octos-org/octos-tui/octos-tui-dev` tracks the latest prerelease.
#
# MUTUALLY EXCLUSIVE with the stable formula: both install a binary named
# `octos-tui`, so only one may be linked at a time (see `conflicts_with` below).
# This is the standard `foo` vs `foo-dev` pattern — install one or the other.
class OctosTuiDev < Formula
  desc "Terminal UI client for the Octos UI Protocol (prerelease channel)"
  homepage "https://github.com/octos-org/octos-tui"
  version "__VERSION__"
  if OS.mac? && Hardware::CPU.arm?
    url "https://github.com/octos-org/octos-tui/releases/download/__TAG__/octos-tui-aarch64-apple-darwin.tar.xz"
    sha256 "__SHA_DARWIN_ARM__"
  end
  if OS.linux?
    if Hardware::CPU.arm?
      url "https://github.com/octos-org/octos-tui/releases/download/__TAG__/octos-tui-aarch64-unknown-linux-gnu.tar.xz"
      sha256 "__SHA_LINUX_ARM__"
    end
    if Hardware::CPU.intel?
      url "https://github.com/octos-org/octos-tui/releases/download/__TAG__/octos-tui-x86_64-unknown-linux-gnu.tar.xz"
      sha256 "__SHA_LINUX_X64__"
    end
  end
  license "Apache-2.0"

  # Dev and stable both provide `bin/octos-tui`; they cannot be linked together.
  # Installing this formula while `octos-tui` is linked (or vice versa) prompts
  # to `brew unlink` the other first, keeping the two channels cleanly separate.
  conflicts_with "octos-tui", because: "both install the octos-tui binary (prerelease vs stable channel)"

  # octos-tui is a CLIENT; a local launch spawns `octos serve --stdio` as its
  # backend. We deliberately do NOT `depends_on "octos-org/octos/octos"`: Homebrew
  # does not auto-tap third-party dependency taps, so that would abort the
  # install with "tap must be installed explicitly". Instead the tui
  # auto-installs the octos server on first run if it's missing (see caveats).
  def caveats
    <<~EOS
      octos-tui-dev is the PRERELEASE (rc/beta) channel; the stable formula is
      `octos-org/octos-tui/octos-tui`. Only one may be linked at a time.

      octos-tui talks to the `octos` server backend. If octos isn't installed,
      octos-tui installs the latest release automatically on first run
      (set OCTOS_TUI_NO_AUTO_INSTALL=1 to disable). To install it up front:
        brew install octos-org/octos/octos
    EOS
  end

  BINARY_ALIASES = {
    "aarch64-apple-darwin":      {},
    "aarch64-unknown-linux-gnu": {},
    "x86_64-pc-windows-gnu":     {},
    "x86_64-unknown-linux-gnu":  {},
  }.freeze

  def target_triple
    cpu = Hardware::CPU.arm? ? "aarch64" : "x86_64"
    os = OS.mac? ? "apple-darwin" : "unknown-linux-gnu"

    "#{cpu}-#{os}"
  end

  def install_binary_aliases!
    BINARY_ALIASES[target_triple.to_sym].each do |source, dests|
      dests.each do |dest|
        bin.install_symlink bin/source.to_s => dest
      end
    end
  end

  def install
    bin.install "octos-tui" if OS.mac? && Hardware::CPU.arm?
    bin.install "octos-tui" if OS.linux? && Hardware::CPU.arm?
    bin.install "octos-tui" if OS.linux? && Hardware::CPU.intel?

    install_binary_aliases!

    # Homebrew will automatically install these, so we don't need to do that
    doc_files = Dir["README.*", "readme.*", "LICENSE", "LICENSE.*", "CHANGELOG.*"]
    leftover_contents = Dir["*"] - doc_files

    # Install any leftover files in pkgshare; these are probably config or
    # sample files.
    pkgshare.install(*leftover_contents) unless leftover_contents.empty?
  end
end
