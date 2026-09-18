api_headers = [
    'source.h',
    'diagnostic.h',
    'text/text.h',
    'text/style.h',
    'syntax/syntax.h',
    'runtime/runtime.h',
    'platform/posix.h',
    'exec/exec.h',
    'library/library.h',
    'library/pure.h',
    'library/bundle.h',
    'session/module.h',
    'session/session.h',
]
pure = static_library(
    'rill-pure',
    'source.c',
    'diagnostic.c',
    'text/text.c',
    'text/style.c',
    'syntax/syntax.c',
    'runtime/heap.c',
    'runtime/code.c',
    'runtime/value.c',
    'runtime/scope.c',
    'runtime/eval.c',
    include_directories: inc,
)
posix = static_library(
    'rill-posix',
    'platform/posix.c',
    'platform/environment.c',
    'exec/launch.c',
    'exec/job.c',
    include_directories: inc,
)
library_core = static_library(
    'rill-library',
    'library/pure.c',
    'library/bundle.c',
    include_directories: inc,
)
shell = executable(
    'rillsh',
    'main.c',
    'session/session.c',
    'session/module.c',
    'library/library.c',
    link_with: [posix, pure, library_core],
    dependencies: yyjson,
    include_directories: inc,
    install: true,
)
