wl_protocols_dir = dependency('wayland-protocols', version: '>=1.37').get_variable('pkgdatadir')
wayland_scanner = find_program(
  dependency('wayland-scanner', native: true).get_variable('wayland_scanner'),
  native: true,
)

protocols = [
  wl_protocols_dir / 'stable/xdg-shell/xdg-shell.xml',
  wl_protocols_dir / 'stable/viewporter/viewporter.xml',
  wl_protocols_dir / 'stable/tablet/tablet-v2.xml',
  wl_protocols_dir / 'staging/single-pixel-buffer/single-pixel-buffer-v1.xml',
  wl_protocols_dir / 'staging/cursor-shape/cursor-shape-v1.xml',
  wl_protocols_dir / 'staging/ext-foreign-toplevel-list/ext-foreign-toplevel-list-v1.xml',
  wl_protocols_dir / 'staging/ext-image-capture-source/ext-image-capture-source-v1.xml',
  wl_protocols_dir / 'staging/ext-image-copy-capture/ext-image-copy-capture-v1.xml',
  # vendored
  meson.current_source_dir() / 'wlr-layer-shell-unstable-v1.xml',
]

protocol_headers = []
protocol_code = []
foreach xml : protocols
  name = fs.stem(xml)
  protocol_headers += custom_target(
    name + '-client-header',
    input: xml,
    output: name + '-client-protocol.h',
    command: [wayland_scanner, 'client-header', '@INPUT@', '@OUTPUT@'],
  )
  protocol_code += custom_target(
    name + '-code',
    input: xml,
    output: name + '-protocol.c',
    command: [wayland_scanner, 'private-code', '@INPUT@', '@OUTPUT@'],
  )
endforeach

protocol_sources = protocol_headers + protocol_code

protocols_lib = static_library(
  'protocols',
  protocol_sources,
  dependencies: wayland_client,
)
protocols_dep = declare_dependency(
  link_with: protocols_lib,
  sources: protocol_headers,
  dependencies: wayland_client,
)

