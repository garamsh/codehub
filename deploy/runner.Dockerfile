# The codehub runner, built from the repository root: docker build -f deploy/runner.Dockerfile .
#
# The runtime stage keeps /bin/sh and coreutils, because the host driver runs a job's command
# under /bin/sh -c. Base images are pinned as in server.Dockerfile, with the same mover.
FROM rust:1.99.1-trixie@sha256:8a4e90c7946b74626488b1446f712c0fe0ed1a596cbd3d871684bc9c5433ae07 AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p codehub-runner \
    && cp target/release/codehub-runner /codehub-runner

FROM debian:trixie-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home --shell /bin/sh codehub
COPY --from=build /codehub-runner /usr/local/bin/codehub-runner
USER 10001
ARG IMAGE_VERSION
ARG IMAGE_REVISION
LABEL org.opencontainers.image.version="${IMAGE_VERSION}" \
      org.opencontainers.image.revision="${IMAGE_REVISION}"
ENTRYPOINT ["/usr/local/bin/codehub-runner"]
