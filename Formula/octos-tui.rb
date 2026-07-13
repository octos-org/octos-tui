class OctosTui < Formula
  desc "Terminal UI client for the Octos UI Protocol"
  homepage "https://github.com/octos-org/octos-tui"
  version "0.2.1"
  if OS.mac? && Hardware::CPU.arm?
    url "https://github.com/octos-org/octos-tui/releases/download/v0.2.1/octos-tui-aarch64-apple-darwin.tar.xz"
    sha256 "1cd65a06cef6b8f6e969be74228aa87481cd55259f1c0998972a29f49f4b42e4"
  end
  if OS.linux?
    if Hardware::CPU.arm?
      url "https://github.com/octos-org/octos-tui/releases/download/v0.2.1/octos-tui-aarch64-unknown-linux-gnu.tar.xz"
      sha256 "80853d88417220994e711dcadd197a7b34f33d8d63b3ab7f695e05fb16269e17"
    end
    if Hardware::CPU.intel?
      url "https://github.com/octos-org/octos-tui/releases/download/v0.2.1/octos-tui-x86_64-unknown-linux-gnu.tar.xz"
      sha256 "f5c73fe15cfd5323fcc6e21f317cf9fbf48703ae140a4deaee053dc50cd872eb"
    end
  end
  license "Apache-2.0"

  # octos-tui is a CLIENT; a local launch spawns `octos serve --stdio` as its
  # backend. We deliberately do NOT `depends_on "octos-org/octos/octos"`: Homebrew
  # does not auto-tap third-party dependency taps, so that would abort the
  # install with "tap must be installed explicitly". Instead the tui
  # auto-installs the octos server on first run if it's missing (see caveats).
  def caveats
    <<~EOS
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
