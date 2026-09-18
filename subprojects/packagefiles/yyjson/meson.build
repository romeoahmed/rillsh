project('yyjson', 'c', version: '0.13.0', license: 'MIT')
yyjson_lib = static_library('yyjson', 'src/yyjson.c')
yyjson = declare_dependency(
    link_with: yyjson_lib,
    include_directories: include_directories('src'),
)
meson.override_dependency('yyjson', yyjson)
