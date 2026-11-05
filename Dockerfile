# Builds valheim-shout and a runtime image with libparty.so baked in.
#
# The `party` stage downloads only the Plugins folder of the Valheim
# dedicated server (Steam app 896660, free with an anonymous login) using
# DepotDownloader, and the runtime stage copies it to /party.

ARG DEPOTDOWNLOADER_VERSION=3.4.0

FROM debian:bookworm-slim AS party
ARG DEPOTDOWNLOADER_VERSION
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl unzip libssl3 \
    && rm -rf /var/lib/apt/lists/*
# DepotDownloader is a self-contained .NET app; skip the ICU dependency.
ENV DOTNET_SYSTEM_GLOBALIZATION_INVARIANT=1
RUN curl -fsSL -o /tmp/dd.zip \
        "https://github.com/SteamRE/DepotDownloader/releases/download/DepotDownloader_${DEPOTDOWNLOADER_VERSION}/DepotDownloader-linux-x64.zip" \
    && unzip -q /tmp/dd.zip -d /opt/depotdownloader \
    && chmod +x /opt/depotdownloader/DepotDownloader \
    && rm /tmp/dd.zip
RUN printf 'regex:^valheim_server_Data/Plugins/\n' > /tmp/files.txt \
    && /opt/depotdownloader/DepotDownloader \
        -app 896660 -os linux -osarch 64 \
        -filelist /tmp/files.txt -dir /valheim \
    && test -f /valheim/valheim_server_Data/Plugins/libparty.so

FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p valheim-shout

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates libpulse0 libpulse-mainloop-glib0 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=party /valheim/valheim_server_Data/Plugins/ /party/
COPY --from=build /src/target/release/valheim-shout /usr/local/bin/valheim-shout
ENV LIBPARTY=/party/libparty.so \
    LD_LIBRARY_PATH=/party
ENTRYPOINT ["valheim-shout"]
