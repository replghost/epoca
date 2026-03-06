# Homebrew Cask for Epoca
#
# To use: copy this file into a tap repo at Casks/epoca.rb
# e.g., github.com/replghost/homebrew-epoca/Casks/epoca.rb
#
# Users install with:
#   brew tap replghost/epoca
#   brew install --cask epoca

cask "epoca" do
  version "0.0.1"

  on_arm do
    url "https://github.com/replghost/epoca/releases/download/v#{version}/Epoca-#{version}-macos-arm64.tar.gz"
    sha256 "PLACEHOLDER_ARM64_SHA256"
  end

  on_intel do
    url "https://github.com/replghost/epoca/releases/download/v#{version}/Epoca-#{version}-macos-x86_64.tar.gz"
    sha256 "PLACEHOLDER_X86_64_SHA256"
  end

  name "Epoca"
  desc "Programmable workbench with sandboxed apps"
  homepage "https://github.com/replghost/epoca"

  app "Epoca.app"

  zap trash: [
    "~/Library/Application Support/Epoca",
    "~/.epoca",
  ]
end
