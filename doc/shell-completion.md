# Shell completion

The Polkadot CLI command supports shell auto-completion. For this to work, you will need to run the completion script matching you build and system.

Assuming you built a release version using `cargo build --release` and use `bash` run the following:

```bash
source target/release/completion-scripts/polkadot.bash
```

You can find completion scripts for:

- bash
- fish
- zsh
- elvish
- powershell

To make this change persistent, you can proceed as follow:

## First install

```bash
COMPL_DIR=$HOME/.completion
mkdir -p $COMPL_DIR
cp -f target/release/completion-scripts/polkadot.bash $COMPL_DIR/
echo "source $COMPL_DIR/polkadot.bash" >> $HOME/.bash_profile
source $HOME/.bash_profile
```

## Update

When you build a new version of Polkadot, the following will ensure you auto-completion script matches the current binary:

```bash
COMPL_DIR=$HOME/.completion
mkdir -p $COMPL_DIR
cp -f target/release/completion-scripts/polkadot.bash $COMPL_DIR/
source $HOME/.bash_profile
```
