# Minimal Ubuntu test host: glibc + GNU coreutils (contracts/integration-testing.md).
# ONLY sshd + a non-privileged account that CANNOT install packages (C-IT1/C-IT2).
FROM ubuntu:24.04

RUN apt-get update \
    && apt-get install -y --no-install-recommends openssh-server \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /run/sshd

# Deterministic host key for stable known_hosts (C-IT3).
COPY testkey/ssh_host_ed25519_key     /etc/ssh/ssh_host_ed25519_key
COPY testkey/ssh_host_ed25519_key.pub /etc/ssh/ssh_host_ed25519_key.pub
RUN chmod 600 /etc/ssh/ssh_host_ed25519_key

# Non-privileged account; no sudo, cannot install packages.
# Unlock (empty password) so pubkey auth is accepted; password login stays disabled.
RUN useradd -m -s /bin/bash tester \
    && sed -i 's/^tester:[^:]*:/tester::/' /etc/shadow
COPY testkey/authorized_keys /home/tester/.ssh/authorized_keys
RUN chown -R tester:tester /home/tester/.ssh && chmod 700 /home/tester/.ssh \
    && chmod 600 /home/tester/.ssh/authorized_keys

RUN sed -i 's/^#\?PasswordAuthentication.*/PasswordAuthentication no/' /etc/ssh/sshd_config \
    && sed -i 's/^#\?PermitRootLogin.*/PermitRootLogin no/' /etc/ssh/sshd_config
EXPOSE 22
CMD ["/usr/sbin/sshd", "-D", "-e"]
