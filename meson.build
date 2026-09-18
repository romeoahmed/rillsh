project(
    'rillsh',
    'c',
    version: '0.1.0',
    license: 'MIT',
    meson_version: '>=1.12.0',
    default_options: {
        'c_std': 'gnu23',
        'warning_level': '3',
        'werror': true,
        'force_fallback_for': ['yyjson'],
    },
)
platform_args = ['-D_POSIX_C_SOURCE=202405L']
if host_machine.system() == 'darwin'
    platform_args += '-D_DARWIN_C_SOURCE'
else
    platform_args += '-D_GNU_SOURCE'
endif
warnings = [
    '-Wconversion',
    '-Wshadow',
    '-Wmissing-prototypes',
    '-Wformat=2',
    '-Wundef',
    '-Wvla',
    '-Wswitch-enum',
    '-Wcast-qual',
    '-Wcast-align',
    '-Wwrite-strings',
    '-Wdouble-promotion',
]
add_project_arguments(warnings, platform_args, language: 'c')
# Keep direct execution and fuzzing fail-fast too, outside Meson's test environment.
if 'undefined' in get_option('b_sanitize').split(',')
    add_project_arguments(
        '-fno-sanitize-recover=undefined',
        language: 'c',
    )
endif
config = configuration_data()
config.set_quoted('RILL_VERSION', meson.project_version())
configure_file(output: 'config.h', configuration: config)
inc = include_directories('.', 'src')
python = find_program('python3', version: '>=3.14', native: true)
yyjson = dependency(
    'yyjson',
    version: '>=0.13.0',
    static: true,
    include_type: 'system',
    default_options: {'werror': true, 'warning_level': '3'},
)
subdir('src')
subdir('tests')
doxygen = find_program('doxygen', required: false, native: true)
if doxygen.found()
    inputs = []
    foreach header : api_headers
        inputs += '"' + header.full_path() + '"'
    endforeach
    doxyfile = configure_file(
        input: 'docs/Doxyfile.in',
        output: 'Doxyfile',
        configuration: {
            'PROJECT_VERSION': meson.project_version(),
            'API_OUTPUT': meson.project_build_root() / 'api',
            'SOURCE_ROOT': meson.project_source_root(),
            'API_INPUT': ' '.join(inputs),
        },
    )
    run_target('api-docs', command: [doxygen, doxyfile])
endif
