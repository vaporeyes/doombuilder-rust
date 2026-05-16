#!/usr/bin/env bash
# ABOUTME: Installs the Ubuntu system packages needed to BUILD the iced/winit
# ABOUTME: GUI on Linux runners. rfd 0.17 uses the XDG portal so no GTK is needed.
set -euo pipefail

sudo apt-get update
# winit links/dlopens xkbcommon + wayland client libs; pkg-config drives the
# build scripts. wgpu loads Vulkan/GL at runtime, so it needs nothing here.
sudo apt-get install -y --no-install-recommends \
  pkg-config \
  libxkbcommon-dev \
  libwayland-dev \
  libxkbcommon-x11-dev
