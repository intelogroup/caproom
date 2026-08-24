class Caproom < Formula
  desc "Memory-cap any command — Rust port, CLI-first v1"
  homepage "https://github.com/intelogroup/caproom"
  url "https://github.com/intelogroup/caproom/archive/refs/tags/v0.8.0.tar.gz"
  sha256 "a5e7dd2e53394449461ac84f4f03f95ba0200c345b6682560cb9fa3485cb6cc8"
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
