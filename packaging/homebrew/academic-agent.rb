class AcademicAgent < Formula
  desc "Single-binary academic task extractor, iCalendar feed, cron reminder, Discord bot, and MCP server"
  homepage "https://github.com/luminariadev/academic-agent"
  version "0.3.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/luminariadev/academic-agent/releases/download/v0.3.0/academic-agent-v0.3.0-aarch64-apple-darwin.tar.gz"
      sha256 "82915fef71b1b7e33c01e21fe4d285cb127b55a2da8c3ca9d6cab1d6a5122231"
    else
      url "https://github.com/luminariadev/academic-agent/releases/download/v0.3.0/academic-agent-v0.3.0-x86_64-apple-darwin.tar.gz"
      sha256 "59c27b614373a0928e5e57a54894a25b9753eb87738fb26043a19381646589aa"
    end
  end

  on_linux do
    if Hardware::CPU.intel?
      url "https://github.com/luminariadev/academic-agent/releases/download/v0.3.0/academic-agent-v0.3.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "4a822fa37017d09ad638bbd856d0bff4d4cead3397c7bf77fb06132e2e6dce05"
    end
  end

  def install
    bin.install "academic-agent"
  end

  test do
    assert_match "academic-agent #{version}", shell_output("#{bin}/academic-agent --version")
  end
end
