api_headers = files(
    'source.h',
    'diagnostic.h',
    'text/text.h',
    'text/style.h',
    'syntax/syntax.h',
    'runtime/runtime.h',
    'platform/posix.h',
    'exec/exec.h',
    'native/native.h',
    'native/pure.h',
    'native/value.h',
    'native/json.h',
    'native/fs.h',
    'native/stream.h',
    'native/bundle.h',
    'session/module.h',
    'session/session.h',
)
pure_sources = files(
    'source.c',
    'diagnostic.c',
    'text/text.c',
    'text/style.c',
    'syntax/parse.c',
    'runtime/heap.c',
    'runtime/prepare.c',
    'runtime/value.c',
    'runtime/scope.c',
    'runtime/eval.c',
)
pure = static_library(
    'rill-pure',
    pure_sources,
    include_directories: inc,
)
posix = static_library(
    'rill-posix',
    'platform/posix.c',
    'platform/environment.c',
    'exec/launch.c',
    'exec/supervisor.c',
    include_directories: inc,
)
native_sources = files(
    'native/pure.c',
    'native/value.c',
    'native/json.c',
    'native/bundle.c',
    'native/dispatch.c',
    'native/fs.c',
    'native/stream.c',
)
native = static_library(
    'rill-native',
    native_sources,
    dependencies: yyjson,
    include_directories: inc,
)
shell = executable(
    'rillsh',
    'main.c',
    'session/session.c',
    'session/evaluation.c',
    'session/module.c',
    link_with: [posix, pure, native],
    include_directories: inc,
    install: true,
)
