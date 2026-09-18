test(
    'unicode-generated',
    python,
    args: [files('../tools/unicode.py'), '--check'],
    suite: 'unicode',
)
foreach name, suites : {
    'text': ['unicode'],
    'syntax': ['syntax'],
    'style': ['editor'],
    'unicode': ['unicode'],
}
    component_test = executable(
        name + '-test',
        'unit/' + name + '.c',
        include_directories: inc,
        link_with: pure,
    )
    test(
        name,
        component_test,
        args: name == 'unicode' ? files('../data/unicode/GraphemeBreakTest.txt') : [],
        suite: suites,
    )
endforeach

foreach name, scenarios : {
    'runtime': ['heap', 'equality', 'entries'],
    'language': ['semantics', 'code', 'collection', 'tail'],
}
    component_test = executable(
        name + '-test',
        'unit/' + name + '.c',
        include_directories: inc,
        link_with: pure,
    )
    foreach scenario : scenarios
        test(
            name + '-' + scenario,
            component_test,
            args: scenario,
            suite: name == 'language' ? ['runtime', 'language'] : ['runtime'],
            timeout: scenario == 'tail' ? 240 : 30,
        )
    endforeach
endforeach

platform_test = executable(
    'platform-test',
    'unit/platform.c',
    include_directories: inc,
    link_with: [posix, pure],
)
test('platform', platform_test, suite: 'platform')
child = executable('test-child', 'helpers/child.c', include_directories: inc)
process_test = executable(
    'process-test',
    'unit/process.c',
    include_directories: inc,
    link_with: [posix, pure],
)
foreach scenario : [
    'streaming',
    'child-state',
    'term-zero',
    'ignore-term',
    'limits',
    'exhaustion',
    'statuses',
]
    test(
        'supervisor-' + scenario,
        process_test,
        args: [child, scenario],
        suite: ['process', 'platform'],
        timeout: 30,
    )
endforeach
test(
    'supervisor-cleanup',
    process_test,
    args: [child, 'cleanup'],
    expected_exitcode: 42,
    suite: ['process', 'platform'],
    timeout: 10,
)
# Substitute failure boundaries only in this archive, never in the installed shell.
fault_subject = static_library(
    'fault-subject',
    '../src/source.c',
    '../src/text/text.c',
    '../src/syntax/syntax.c',
    '../src/runtime/heap.c',
    '../src/runtime/code.c',
    '../src/runtime/value.c',
    '../src/runtime/scope.c',
    '../src/runtime/eval.c',
    '../src/library/library.c',
    '../src/library/pure.c',
    '../src/platform/environment.c',
    '../src/exec/launch.c',
    include_directories: inc,
    c_args: [
        '-Dgetcwd=rill_test_getcwd',
        '-Dfchdir=rill_test_fchdir',
        '-Dfork=rill_test_fork',
        '-Dsetpgid=rill_test_setpgid',
        '-Dmalloc=rill_test_malloc',
        '-Dcalloc=rill_test_calloc',
        '-Drealloc=rill_test_realloc',
        '-Dstrdup=rill_test_strdup',
    ],
)
fault_test = executable(
    'fault-test',
    'unit/faults.c',
    include_directories: inc,
    link_with: [fault_subject, posix, pure, library_core],
)
foreach scenario, suites : {
    'memory': ['syntax', 'runtime'],
    'library': ['language'],
    'directory': ['platform'],
    'launch': ['platform'],
}
    test(
        'faults-' + scenario,
        fault_test,
        args: [child, scenario],
        suite: ['fault'] + suites,
        timeout: 30,
    )
endforeach
foreach name, suites : {
    'shell': ['process', 'platform'],
    'terminal': ['pty'],
    'language': ['language'],
}
    test(
        name + '-system',
        python,
        args: [
            '-m',
            'unittest',
            'discover',
            '-s',
            meson.current_source_dir() / 'system',
            '-p',
            'test_' + name + '.py',
            '-v',
        ],
        env: {
            'RILL_TEST_SHELL': shell.full_path(),
            'RILL_TEST_CHILD': child.full_path(),
        },
        depends: [shell, child],
        suite: suites,
        timeout: 90,
    )
endforeach

library_test = executable(
    'library-test',
    'unit/library.c',
    include_directories: inc,
    link_with: [pure, library_core],
)
test('library-stress', library_test, suite: ['runtime', 'language'])

# Benchmarks are separate from correctness tests and run serially through Meson.
foreach name : [
    'tail',
    'curry',
    'equal',
    'text',
    'map',
    'closures',
    'dag',
    'records',
]
    benchmark(
        'runtime-' + name,
        shell,
        args: files('benchmarks/' + name + '.rill'),
        timeout: 120,
    )
endforeach

foreach name : ['syntax', 'gc', 'bindings', 'captures', 'sharing']
    component_benchmark = executable(
        name + '-benchmark',
        'benchmarks/' + name + '.c',
        include_directories: inc,
        link_with: pure,
    )
    benchmark('runtime-' + name, component_benchmark, timeout: 120)
endforeach
