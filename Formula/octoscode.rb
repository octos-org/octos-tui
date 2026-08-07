class Octoscode < Formula
  desc "Terminal UI client for the Octos UI Protocol"
  homepage "https://github.com/octos-org/octoscode"
  version "0.3.0"
  if OS.mac? && Hardware::CPU.arm?
    url "https://github.com/octos-org/octoscode/releases/download/v0.3.0/octoscode-aarch64-apple-darwin.tar.xz"
    sha256 "c8c6321018d6576e5db6e41f9decdf45c5cca92788c3558124aaa175039b1ae0"
  end
  if OS.linux?
    if Hardware::CPU.arm?
      url "https://github.com/octos-org/octoscode/releases/download/v0.3.0/octoscode-aarch64-unknown-linux-gnu.tar.xz"
      sha256 "c411e5649c42ab349f1e1fb3b75c551078b12dcf23d7e97e61ae82515d374f9d"
    end
    if Hardware::CPU.intel?
      url "https://github.com/octos-org/octoscode/releases/download/v0.3.0/octoscode-x86_64-unknown-linux-gnu.tar.xz"
      sha256 "8f20bce688063769baa523ff3a7b23d748aa8e908530e6e6378607eb34e7bb01"
    end
  end
  license "Apache-2.0"

  # octoscode is a CLIENT; a local launch spawns `octos serve --stdio` as its
  # backend. We deliberately do NOT `depends_on "octos-org/octos/octos"`: Homebrew
  # does not auto-tap third-party dependency taps, so that would abort the
  # install with "tap must be installed explicitly". Instead the tui
  # auto-installs the octos server on first run if it's missing (see caveats).
  def caveats
    <<~EOS
      octoscode talks to the `octos` server backend. If octos isn't installed,
      octoscode installs the latest release automatically on first run
      (set OCTOSCODE_NO_AUTO_INSTALL=1 to disable). To install it up front:
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
    bin.install "octoscode" if OS.mac? && Hardware::CPU.arm?
    bin.install "octoscode" if OS.linux? && Hardware::CPU.arm?
    bin.install "octoscode" if OS.linux? && Hardware::CPU.intel?

    install_binary_aliases!

    # Homebrew will automatically install these, so we don't need to do that
    doc_files = Dir["README.*", "readme.*", "LICENSE", "LICENSE.*", "CHANGELOG.*"]
    leftover_contents = Dir["*"] - doc_files

    # Install any leftover files in pkgshare; these are probably config or
    # sample files.
    pkgshare.install(*leftover_contents) unless leftover_contents.empty?
  end
end
