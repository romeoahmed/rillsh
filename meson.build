project(
    'rillsh',
    'c',
    version: '0.1.0',
    license: 'MIT',
    meson_version: '>=1.12.0',
    default_options: [
        'c_std=gnu23',
        'warning_level=3',
        'werror=true',
        'force_fallback_for=yyjson',
    ],
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
if get_option('b_sanitize') != 'none'
    add_project_arguments(
        '-fno-sanitize-recover=undefined',
        language: 'c',
    )
endif
config = configuration_data()
config.set_quoted('RILL_VERSION', meson.project_version())
configure_file(output: 'config.h', configuration: config)
inc = include_directories('.', 'src')
python = import('python').find_installation('python3')
if not python.language_version().version_compare('>=3.14')
    error('Development tools and tests require Python 3.14 or newer.')
endif
yyjson = dependency(
    'yyjson',
    version: '>=0.13.0',
    static: true,
    default_options: ['werror=true', 'warning_level=3'],
)
subdir('src')
subdir('tests')
doxygen = find_program('doxygen', required: false)
if doxygen.found()
    doxy = configuration_data()
    doxy.set('PROJECT_VERSION', meson.project_version())
    doxy.set('API_OUTPUT', meson.project_build_root() / 'api')
    doxy.set('SOURCE_ROOT', meson.project_source_root())
    inputs = []
    foreach header : api_headers
        inputs += '"' + meson.project_source_root() / 'src' / header + '"'
    endforeach
    doxy.set('API_INPUT', ' '.join(inputs))
    doxyfile = configure_file(
        input: 'docs/Doxyfile.in',
        output: 'Doxyfile',
        configuration: doxy,
    )
    run_target('api-docs', command: [doxygen, doxyfile])
endif
run_target(
    'check',
    command: [python, files('tools/check.py'), meson.project_build_root()],
)
summary(
    {
        'Unicode': '18.0.0 (pinned inputs)',
        'yyjson': yyjson.version(),
        'API documentation': doxygen.found(),
        'Profile': get_option('buildtype'),
        'Sanitizers': get_option('b_sanitize'),
    },
    section: 'Rill Shell',
)
