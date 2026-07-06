# Minimal Debian test host: glibc + GNU coreutils (contracts/integration-testing.md).
# ONLY sshd + a non-privileged account that CANNOT install packages, so the test
# honestly exercises zero-footprint and no-root operation (C-IT1/C-IT2).
FROM debian:bookworm-slim
ARG KEYDIR=testkey

# openssh-server is the ONLY thing we add; no shells-as-plugins, no extra tools.
RUN apt-get update \
    && apt-get install -y --no-install-recommends openssh-server \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /run/sshd

# Deterministic host key for stable known_hosts in tests (C-IT3).
COPY ${KEYDIR}/ssh_host_ed25519_key     /etc/ssh/ssh_host_ed25519_key
COPY ${KEYDIR}/ssh_host_ed25519_key.pub /etc/ssh/ssh_host_ed25519_key.pub
RUN chmod 600 /etc/ssh/ssh_host_ed25519_key

# Non-privileged account; NO sudo, NOT in any admin group → cannot install packages.
# Unlock (empty password) so pubkey auth is accepted; password login stays disabled.
RUN useradd -m -s /bin/bash tester \
    && sed -i 's/^tester:[^:]*:/tester::/' /etc/shadow
COPY ${KEYDIR}/authorized_keys /home/tester/.ssh/authorized_keys
RUN chown -R tester:tester /home/tester/.ssh && chmod 700 /home/tester/.ssh \
    && chmod 600 /home/tester/.ssh/authorized_keys

# Key-only auth, known port.
RUN sed -i 's/^#\?PasswordAuthentication.*/PasswordAuthentication no/' /etc/ssh/sshd_config \
    && sed -i 's/^#\?PermitRootLogin.*/PermitRootLogin no/' /etc/ssh/sshd_config
EXPOSE 22
CMD ["/usr/sbin/sshd", "-D", "-e"]
