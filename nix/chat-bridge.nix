{ lib, buildGoModule }:
# The Chat bridge (next/bridge): whatsmeow behind a stdio protocol for the Node runtime.
buildGoModule {
  pname = "dar-chat-bridge";
  version = "0.1.0";
  src = lib.cleanSourceWith { src = ../next/bridge; filter = path: type: !(lib.hasSuffix "_test.go" (baseNameOf path)); };
  vendorHash = "sha256-nPda8UhA6MpQ9oY7g9bUZolTcPBtZEMO6kN561bOfpc=";
  env.CGO_ENABLED = "0";
  ldflags = [ "-s" "-w" ];
  postInstall = ''mv $out/bin/chat-bridge $out/bin/dar-chat-bridge'';
  meta = { description = "WhatsApp bridge for Dar's Chat app"; license = lib.licenses.mpl20; platforms = [ "x86_64-linux" "aarch64-linux" ]; mainProgram = "dar-chat-bridge"; };
}
