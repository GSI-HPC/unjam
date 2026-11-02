# SPDX-License-Identifier: LGPL-3.0-or-later
# SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

# Packages the static musl binary of a GitHub release as it is: the file in /usr/bin is the
# file that the release attests (`gh attestation verify /usr/bin/unjam -R GSI-HPC/unjam`).
# scripts/rpm.sh defines ver and puts the binary and the docs in SOURCES.

# No compiler runs: no debuginfo, no build-id links, no strip or other brp step on the binary.
%global debug_package %{nil}
%global _build_id_links none
%global __os_install_post %{nil}

Name:           unjam
Version:        %{ver}
Release:        1%{?dist}
Summary:        Utility for Node/Job Analysis and Monitoring
License:        LGPL-3.0-or-later
URL:            https://github.com/GSI-HPC/unjam
Source0:        unjam
Source1:        LGPL-3.0-or-later.txt
Source2:        README.md
Source3:        CHANGELOG.md
ExclusiveArch:  x86_64 aarch64

%description
unjam is a top-like terminal UI over the cgroup v2 tree of a Linux host. It shows CPU,
memory, IO and PIDS for each cgroup and process, attaches to the nodes of a Slurm job
(unjam --slurm), and records to and replays from Arrow IPC files.

%prep
cp -p %{SOURCE1} %{SOURCE2} %{SOURCE3} .

%build

%install
install -Dpm 0755 %{SOURCE0} %{buildroot}%{_bindir}/unjam

%files
%license LGPL-3.0-or-later.txt
%doc README.md CHANGELOG.md
%{_bindir}/unjam
