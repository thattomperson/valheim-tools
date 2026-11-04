# Builds valheim-shout and a runtime image that can load libparty.so.
# libparty.so is not included: mount it from a Valheim dedicated server
# install (valheim_server_Data/Plugins/libparty.so) at /party/libparty.so.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p valheim-shout

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates libpulse0 libpulse-mainloop-glib0 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/valheim-shout /usr/local/bin/valheim-shout
ENV LIBPARTY=/party/libparty.so
ENTRYPOINT ["valheim-shout"]
