# SPDX-License-Identifier: LGPL-3.0-or-later
# SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

# The repository configuration and the public signing key. Its version follows unjam, so that
# each release adds one unjam-release per distribution and two builds never share a name.

%{!?repo_url:%global repo_url https://gsi-hpc.github.io/unjam/rpm}
%if 0%{?fedora}
%global repo_path fedora/$releasever
%global repo_name Fedora $releasever
%else
%global repo_path el/%{rhel}
%global repo_name Enterprise Linux %{rhel}
%endif

Name:           unjam-release
Version:        %{ver}
Release:        1%{?dist}
Summary:        Package repository configuration for unjam
License:        LGPL-3.0-or-later
URL:            https://github.com/GSI-HPC/unjam
Source0:        RPM-GPG-KEY-unjam
BuildArch:      noarch

%description
The dnf repository configuration and the public signing key for the unjam packages
from %{repo_url}/%{repo_path}.

%prep

%build

%install
install -Dpm 0644 %{SOURCE0} %{buildroot}%{_sysconfdir}/pki/rpm-gpg/RPM-GPG-KEY-unjam
mkdir -p %{buildroot}%{_sysconfdir}/yum.repos.d
cat > %{buildroot}%{_sysconfdir}/yum.repos.d/unjam.repo <<'REPO'
[unjam]
name=unjam for %{repo_name} - $basearch
baseurl=%{repo_url}/%{repo_path}/$basearch/
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-unjam
metadata_expire=6h
REPO

%files
%config(noreplace) %{_sysconfdir}/yum.repos.d/unjam.repo
%{_sysconfdir}/pki/rpm-gpg/RPM-GPG-KEY-unjam
