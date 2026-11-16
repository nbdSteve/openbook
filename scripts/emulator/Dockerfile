# AWS Emulator, built from the latest LTS release RPM.
#
# Never pin a version. To upgrade, rebuild with:
#   docker build --pull --no-cache -t aws-emulator scripts/emulator
FROM public.ecr.aws/amazonlinux/amazonlinux:2023-minimal

ARG TARGETARCH
RUN case "$TARGETARCH" in \
      arm64) arch=aarch64 ;; \
      amd64) arch=x86_64 ;; \
      *) echo "unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac \
    && curl -fsSLo /tmp/aws-emulator.rpm \
      "https://downloads.aws.com/aws-emulator/packages/aws-emulator/LTS/latest/aws-emulator-${arch}.rpm" \
    && rpm -i /tmp/aws-emulator.rpm \
    && rm /tmp/aws-emulator.rpm \
    && mkdir -p /home/emulator \
    && chown 1000 /home/emulator \
    # Any user may run the container (CI uses the runner uid), so HOME must be writable.
    && chmod 777 /home/emulator

USER 1000
ENV HOME=/home/emulator
# Telemetry is off unless you start the container with -e EMULATOR_TELEMETRY=on.
ENV EMULATOR_TELEMETRY=off
ENV PATH=/opt/aws-emulator/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
EXPOSE 9297
# `-b 0.0.0.0` binds inside the container. scripts/emulator.sh publishes the port on
# 127.0.0.1 only.
ENTRYPOINT ["aws-emulator", "-b", "0.0.0.0"]
