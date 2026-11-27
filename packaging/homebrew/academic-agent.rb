class AcademicAgent < Formula
  desc "Single-binary academic task extractor, iCalendar feed, cron reminder, Discord bot, and MCP server"
  homepage "https://github.com/luminariadev/academic-agent"
  version "0.2.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/luminariadev/academic-agent/releases/download/v0.2.0/academic-agent-v0.2.0-aarch64-apple-darwin.tar.gz"
      sha256 "ed87f66f47ba61e2d719a1603cde8a90fde986b1609a45a8e3ea1884ec3a5ac9"
    else
      url "https://github.com/luminariadev/academic-agent/releases/download/v0.2.0/academic-agent-v0.2.0-x86_64-apple-darwin.tar.gz"
      sha256 "20b6f45a93df16ac2bcde9495fdb38355355e6d2e59bfbca6449fd7b4b2e38f1"
    end
  end

  on_linux do
    if Hardware::CPU.intel?
      url "https://github.com/luminariadev/academic-agent/releases/download/v0.2.0/academic-agent-v0.2.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "6704fc5bd7299c960f1438a29536e533839b45316f3a3efd90730b95fa791d60"
    end
  end

  def install
    bin.install "academic-agent"
  end

  test do
    assert_match "academic-agent #{version}", shell_output("#{bin}/academic-agent --version")
  end
end
