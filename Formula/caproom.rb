class Caproom < Formula
  desc "Memory-cap any command — Rust port, CLI-first v1"
  homepage "https://github.com/intelogroup/caproom"
  url "https://github.com/intelogroup/caproom/archive/refs/tags/v0.8.0.tar.gz"
  sha256 "REPLACE_WITH_SHA256_OF_TARBALL"
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
