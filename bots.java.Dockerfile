# Image for the Java example bots (examples/java).
# Built once and shared by every Java bot service in docker-compose.bots.yml.
#
# Uses a per-Dockerfile ignore file (bots.java.Dockerfile.dockerignore) so the
# build context keeps sdk-java/ and examples/ — the root .dockerignore drops them.

# ---- build stage: install the SDK, compile the bots, collect dependencies ----
FROM maven:3.9-eclipse-temurin-17 AS build
WORKDIR /build

# Build and install naval-sdk into the local Maven repo so the examples can
# resolve it. Tests are skipped — this image only needs the runnable artefacts.
COPY sdk-java/ ./sdk-java/
RUN cd sdk-java && mvn -q -B -DskipTests install

# Compile the example bots and copy their runtime dependency jars into lib/.
COPY examples/java/ ./examples-java/
RUN cd examples-java \
    && mvn -q -B compile \
    && mvn -q -B dependency:copy-dependencies \
       -DoutputDirectory=lib -DincludeScope=runtime

# ---- runtime stage: JRE + compiled classes + dependency jars ----
FROM eclipse-temurin:17-jre
WORKDIR /app
COPY --from=build /build/examples-java/target/classes ./classes
COPY --from=build /build/examples-java/lib ./lib

# Run as: java -cp classes:lib/* <main-class> <host> <port> <name>
# Java expands the lib/* classpath wildcard itself (no shell involved).
ENTRYPOINT ["java", "-cp", "classes:lib/*"]
