# bash completion for xxh. Load it with:  eval "$(xxh completions bash)"
#
# A thin stub: the candidates come from `xxh __complete`, so they always match
# the installed xxh.

_xxh() {
    local line=$COMP_LINE words=() cword=0 i w lead prev=
    # Bash cuts words at ':' (COMP_WORDBREAKS), but targets look like
    # `docker:app`: glue the pieces that touch each other back together.
    for i in "${!COMP_WORDS[@]}"; do
        w=${COMP_WORDS[i]}
        lead=x
        if [[ -n $w ]]; then
            lead=${line%%"$w"*}
            line=${line#*"$w"}
        fi
        if [[ ${#words[@]} -gt 1 && -z $lead && ( $w == :* || $prev == *: ) ]]; then
            words[${#words[@]}-1]+=$w
        else
            words+=("$w")
        fi
        prev=$w
        [[ $i -eq $COMP_CWORD ]] && cword=$((${#words[@]} - 1))
    done

    local out cur=${words[cword]} strip= item
    COMPREPLY=()
    out=$("${words[0]}" __complete bash "$cword" -- "${words[@]}" 2>/dev/null) || return 0
    # Bash replaces only what follows the last ':' of the word.
    if [[ $COMP_WORDBREAKS == *:* && $cur == *:* ]]; then
        strip=${cur%"${cur##*:}"}
    fi
    while IFS= read -r item; do
        item=${item%%$'\t'*}
        [[ -n $item ]] && COMPREPLY+=("${item#"$strip"}")
    done <<<"$out"
    # `docker:`, `user@` and directories are continued, not finished.
    if [[ ${#COMPREPLY[@]} -eq 1 && ${COMPREPLY[0]} == *[:@/] ]]; then
        compopt -o nospace 2>/dev/null
    fi
    return 0
}

complete -F _xxh xxh
