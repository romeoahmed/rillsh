test(
    'unicode-generated',
    python,
    args: [files('../tools/unicode.py'), '--check'],
    suite: 'unicode',
)
test(
    'unicode-generator',
    python,
    args: [
        '-m',
        'unittest',
        'discover',
        '-s',
        'tests/unit',
        '-p',
        'test_unicode.py',
        '-v',
    ],
    workdir: meson.project_source_root(),
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
            timeout: scenario == 'tail' ? 240 : default(),
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
child = executable('test-child', 'support/child.c', include_directories: inc)
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
    pure_sources,
    native_sources,
    '../src/platform/environment.c',
    '../src/exec/launch.c',
    include_directories: inc,
    dependencies: yyjson,
    c_args: [
        '-Dgetcwd=rill_test_getcwd',
        '-Dfchdir=rill_test_fchdir',
        '-Dfork=rill_test_fork',
        '-Dsetpgid=rill_test_setpgid',
        '-Dmalloc=rill_test_malloc',
        '-Dcalloc=rill_test_calloc',
        '-Drealloc=rill_test_realloc',
        '-Dstrdup=rill_test_strdup',
        '-Dstrndup=rill_test_strndup',
    ],
)
fault_test = executable(
    'fault-test',
    'unit/faults.c',
    '../src/session/module.c',
    include_directories: inc,
    link_with: [fault_subject, posix],
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
    )
endforeach
foreach name, suites : {
    'shell': ['process', 'platform'],
    'terminal': ['pty'],
    'language': ['language'],
    'streams': ['stream', 'process'],
    'codecs': ['codec'],
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

native_test = executable(
    'native-test',
    'unit/native.c',
    '../src/session/module.c',
    include_directories: inc,
    link_with: [pure, native, posix],
)
test('native-stress', native_test, suite: ['runtime', 'language'])

subdir('benchmarks')
if get_option('fuzz')
    subdir('fuzz')
endif
