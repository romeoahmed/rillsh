test(
    'unicode-generated',
    python,
    args: [files('../tools/unicode.py'), '--check'],
    suite: 'unicode',
)
foreach name, suites : {
    'text': ['unicode'],
    'syntax': ['syntax'],
    'runtime': ['runtime'],
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
    link_with: [fault_subject, posix, pure],
)
foreach scenario : ['memory', 'directory', 'launch']
    test(
        'faults-' + scenario,
        fault_test,
        args: [child, scenario],
        suite: ['fault', scenario == 'memory' ? 'syntax' : 'platform'],
        timeout: 30,
    )
endforeach
foreach name : ['shell', 'terminal']
    test(
        name,
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
        suite: name == 'terminal' ? ['pty'] : ['process', 'platform'],
        timeout: 90,
    )
endforeach
