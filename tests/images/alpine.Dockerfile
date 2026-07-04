# CRITICAL test host: Alpine — musl + BusyBox (contracts/integration-testing.md C-IT4).
# Minimal POSIX sh (BusyBox), trimmed uname/coreutils, no glibc. bootstrap.sh and
# platform-detection MUST work here. ONLY sshd + non-privileged account (C-IT1/C-IT2).
FROM alpine:3.20

RUN apk add --no-cache openssh-server \
    && ssh-keygen -A >/dev/null 2>&1 || true

# Deterministic host key overrides the generated one for stable known_hosts (C-IT3).
COPY testkey/ssh_host_ed25519_key     /etc/ssh/ssh_host_ed25519_key
COPY testkey/ssh_host_ed25519_key.pub /etc/ssh/ssh_host_ed25519_key.pub
RUN chmod 600 /etc/ssh/ssh_host_ed25519_key

# Non-privileged account with BusyBox /bin/sh; not in wheel → cannot use apk/su to root.
# Unlock the account (empty password field) so pubkey auth is accepted; password
# login stays disabled via `PasswordAuthentication no`.
RUN adduser -D -s /bin/sh tester \
    && sed -i 's/^tester:[^:]*:/tester::/' /etc/shadow
COPY testkey/authorized_keys /home/tester/.ssh/authorized_keys
RUN chown -R tester:tester /home/tester/.ssh && chmod 700 /home/tester/.ssh \
    && chmod 600 /home/tester/.ssh/authorized_keys

RUN sed -i 's/^#\?PasswordAuthentication.*/PasswordAuthentication no/' /etc/ssh/sshd_config \
    && sed -i 's/^#\?PermitRootLogin.*/PermitRootLogin no/' /etc/ssh/sshd_config
EXPOSE 22
CMD ["/usr/sbin/sshd", "-D", "-e"]
