# fish completion for xxh. Load it with:  xxh completions fish | source
#
# A thin stub: the candidates come from `xxh __complete`, so they always match
# the installed xxh.

function __xxh_complete
    set -l words (commandline --current-process --tokenize --cut-at-cursor) (commandline --current-token)
    $words[1] __complete fish (math (count $words) - 1) -- $words 2>/dev/null
end

complete --command xxh --exclusive --keep-order --arguments '(__xxh_complete)'
