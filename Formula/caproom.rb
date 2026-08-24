class Caproom < Formula
  desc "Memory-cap any command — Rust port, CLI-first v1"
  homepage "https://github.com/intelogroup/caproom"
  url "https://github.com/intelogroup/caproom/archive/refs/tags/v0.9.0.tar.gz"
  sha256 "4090fc83f78d9022d6bffc6775452296912c146f51d30ae6b87170aa7c621ae6"
  license "MIT"
  depends_on "rust" => :build
  def install
    system "cargo", "install", "--locked", "--path", "crates/cli", "--root", prefix
  end
  test do
    system "#{bin}/caproom", "--help"
    system "#{bin}/caproom", "calibrate", "--duration", "0"
  end
end
