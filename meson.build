project(
  'wl-expose',
  'c',
  version: '0.1.0',
  meson_version: '>=0.60.0',
  default_options: [
    'c_std=c11',
    'warning_level=2',
    'buildtype=debugoptimized',
  ],
)

add_project_arguments(
  '-D_GNU_SOURCE',
  '-DWL_EXPOSE_VERSION="@0@"'.format(meson.project_version()),
  language: 'c',
)

fs = import('fs')
cc = meson.get_compiler('c')

wayland_client = dependency('wayland-client')
xkbcommon = dependency('xkbcommon')
math = cc.find_library('m', required: false)

subdir('protocol')

executable(
  'wl-expose',
  'src/main.c',
  'src/wl.c',
  'src/toplevel.c',
  'src/capture.c',
  'src/layout.c',
  'src/overlay.c',
  'src/input.c',
  dependencies: [protocols_dep, xkbcommon, math],
  install: true,
)

src_inc = include_directories('src')

test(
  'layout',
  executable(
    'test-layout',
    'tests/test_layout.c',
    'src/layout.c',
    include_directories: src_inc,
    dependencies: math,
  ),
)

test(
  'toplevel',
  executable(
    'test-toplevel',
    'tests/test_toplevel.c',
    'src/toplevel.c',
    include_directories: src_inc,
    dependencies: protocols_dep,
  ),
)

