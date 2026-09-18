FROM quay.io/fedora/fedora:45
RUN dnf -y install --refresh gcc clang clang-tools-extra compiler-rt llvm meson ninja-build \
      doxygen python3 glibc-devel git curl ca-certificates && \
    dnf clean all
WORKDIR /work
CMD ["/bin/bash"]
