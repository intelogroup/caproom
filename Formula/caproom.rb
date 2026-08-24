class Caproom < Formula
  desc "Memory-cap any command — Rust port, CLI-first v1"
  homepage "https://github.com/intelogroup/caproom"
  url "https://github.com/intelogroup/caproom/archive/refs/tags/v0.8.1.tar.gz"
  sha256 "65c6ecc7a3d22d70b1a224267cd71328c1ea24a0a143ff169ba1fb8b5199fe83"
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
