class Kaze < Formula
  desc "Ultra-strict systems programming language for AI and machines"
  homepage "https://github.com/sofuu-runtime/kaze"
  url "https://github.com/sofuu-runtime/kaze/archive/refs/tags/v1.0.0.tar.gz"
  license "MIT"

  depends_on "make" => :build

  def install
    system "cc", "-O3", "-I", "dist", "dist/kaze.c", "dist/kaze_rt.c", "-lm", "-o", "bin/kaze"
    bin.install "bin/kaze"
  end

  test do
    (testpath/"test.kaze").write <<~EOS
      action main():
          return 42
    EOS
    assert_match "42", shell_output("#{bin}/kaze run test.kaze")
  end
end
