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
    'unwind',
    'updates',
    'streams',
    'patterns',
    'aggregates',
    'lookup',
    'lines',
]
    benchmark(
        'runtime-' + name,
        shell,
        args: files(name + '.rill'),
        suite: 'language',
        timeout: 120,
    )
endforeach

foreach name : [
    'syntax',
    'gc',
    'bindings',
    'captures',
    'dag_equal',
    'escape',
    'snapshots',
    'functions',
]
    component_benchmark = executable(
        name.replace('_', '-') + '-benchmark',
        name + '.c',
        include_directories: inc,
        link_with: pure,
        build_by_default: false,
    )
    benchmark(
        'runtime-' + name.replace('_', '-'),
        component_benchmark,
        suite: 'runtime',
        timeout: 120,
    )
    if name == 'syntax'
        foreach shape : ['records', 'strings']
            benchmark(
                'syntax-' + shape,
                component_benchmark,
                args: shape,
                suite: 'runtime',
                timeout: 120,
            )
        endforeach
    endif
endforeach

json_benchmark = executable(
    'json-benchmark',
    'json.c',
    include_directories: inc,
    link_with: [pure, native],
    build_by_default: false,
)
foreach shape : ['wide', 'deep']
    foreach operation : ['decode', 'encode']
        benchmark(
            'json-' + operation + '-' + shape,
            json_benchmark,
            args: [shape, operation],
            suite: 'codec',
            timeout: 120,
        )
    endforeach
endforeach

preparation_benchmark = executable(
    'preparation-benchmark',
    'preparation.c',
    include_directories: inc,
    link_with: pure,
    build_by_default: false,
)
foreach shape : ['nested', 'literals', 'unique']
    benchmark(
        'prepare-' + shape,
        preparation_benchmark,
        args: shape,
        suite: 'preparation',
        timeout: 120,
    )
endforeach
