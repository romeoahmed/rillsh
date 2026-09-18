doxygen = find_program('doxygen', required: false, native: true)
if doxygen.found()
    inputs = []
    foreach header : api_headers
        inputs += '"' + header.full_path() + '"'
    endforeach
    doxyfile = configure_file(
        input: 'Doxyfile.in',
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
