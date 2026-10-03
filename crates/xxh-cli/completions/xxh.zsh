#compdef xxh
# zsh completion for xxh. Load it with:  source <(xxh completions zsh)
# (after compinit), or install this file as `_xxh` in a directory of $fpath.
#
# A thin stub: the candidates come from `xxh __complete`, so they always match
# the installed xxh.

_xxh() {
    local -a lines described plain open
    local line value desc
    lines=("${(@f)$("${words[1]}" __complete zsh $((CURRENT - 1)) -- "${words[@]}" 2>/dev/null)}")
    for line in $lines; do
        value=${line%%$'\t'*}
        desc=
        [[ $line == *$'\t'* ]] && desc=${line#*$'\t'}
        [[ -n $value ]] || continue
        if [[ $value == *[:@/] ]]; then
            # `docker:`, `user@` and directories are continued, not finished.
            open+=("$value")
        elif [[ -n $desc ]]; then
            described+=("${value//:/\\:}:$desc")
        else
            plain+=("$value")
        fi
    done
    (( $#described + $#plain + $#open )) || return 1
    (( $#described )) && _describe -t commands command described
    (( $#plain )) && compadd -a plain
    (( $#open )) && compadd -S '' -a open
    return 0
}

if [[ $funcstack[1] == _xxh ]]; then
    _xxh "$@"
elif (( $+functions[compdef] )); then
    compdef _xxh xxh
fi
