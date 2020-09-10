%define debug_package %{nil}

Name: polkadot
Summary: Implementation of a https://polkadot.network node in Rust based on the Substrate framework.
Version: @@VERSION@@
Release: @@RELEASE@@%{?dist}
License: GPLv3
Group: Applications/System
Source0: %{name}-%{version}.tar.gz

Requires: systemd, shadow-utils
Requires(post): systemd
Requires(preun): systemd
Requires(postun): systemd

BuildRoot: %{_tmppath}/%{name}-%{version}-%{release}-root

%description
%{summary}


%prep
%setup -q


%install
rm -rf %{buildroot}
mkdir -p %{buildroot}
cp -a * %{buildroot}

%post
config_file="/etc/default/polkadot"
getent group polkadot >/dev/null || groupadd -r polkadot
getent passwd polkadot >/dev/null || \
    useradd -r -g polkadot -d /home/polkadot -m -s /sbin/nologin \
    -c "User account for running polkadot as a service" polkadot
if [ ! -e "$config_file" ]; then
    echo 'POLKADOT_CLI_ARGS=""' > /etc/default/polkadot
fi
exit 0

%clean
rm -rf %{buildroot}

%files
%defattr(-,root,root,-)
%{_bindir}/*
/usr/lib/systemd/system/polkadot.service
