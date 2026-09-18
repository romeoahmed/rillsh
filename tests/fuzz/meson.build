assert(
    'fuzzer-no-link' in get_option('b_sanitize').split(','),
    'Fuzz builds require -Db_sanitize=address,undefined,fuzzer-no-link to instrument linked libraries.',
)
foreach name, seeds : {
    'json': ['values.json', 'duplicates.json', 'boundaries.json'],
    'lines': [
        'endings.txt',
        'empty.txt',
        'blank.txt',
        'crlf.txt',
        'utf8.txt',
        'invalid-utf8.txt',
    ],
    'syntax': [
        'closures.rill',
        'recursive.rill',
        'patterns.rill',
        'multiline.rill',
        'invalid.rill',
        'incomplete.rill',
    ],
}
    libraries = [pure]
    if name == 'json'
        libraries += native
    elif name == 'lines'
        libraries += [posix, native]
    endif
    fuzzer = executable(
        name + '-fuzz',
        name + '.c',
        link_with: libraries,
        include_directories: inc,
        link_args: ['-fsanitize=fuzzer'],
    )
    inputs = []
    foreach seed : seeds
        inputs += files('corpus' / name / seed)
    endforeach
    # Explicit files replay once; mutation campaigns stay outside the gate.
    test('fuzz-' + name + '-seeds', fuzzer, args: inputs, suite: 'fuzz')
endforeach
