class Caproom < Formula
  desc "Memory-cap any command — Rust port, CLI-first v1"
  homepage "https://github.com/intelogroup/caproom"
  url "https://github.com/intelogroup/caproom/archive/refs/tags/v0.8.2.tar.gz"
  sha256 "2197de18410e82b24bc8cc4cd0d909f623b63e6626a2f24f9aaf48445d13ed48"
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
