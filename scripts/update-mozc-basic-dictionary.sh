#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "$0")/.." && pwd)"
output_file="$workspace_dir/crates/ime-converter/data/mozc-basic.tsv"
connection_output_file="$workspace_dir/crates/ime-converter/data/mozc-connection.bin"
mozc_revision="3f235b4eb6fcff7d14ef5f0fb8ee56de7ee4c732"
dictionary_cost_threshold=8500
expected_entries=1085464
expected_connection_bytes=3553636

temporary_dir="$(mktemp -d)"
trap 'rm -rf "$temporary_dir"' EXIT

git clone --quiet --filter=blob:none --no-checkout https://github.com/google/mozc.git "$temporary_dir/mozc"
git -C "$temporary_dir/mozc" sparse-checkout set src/data/dictionary_oss
git -C "$temporary_dir/mozc" checkout --quiet "$mozc_revision"

dictionary_dir="$temporary_dir/mozc/src/data/dictionary_oss"
temporary_output="$temporary_dir/mozc-basic.tsv"

awk -F '\t' -v threshold="$dictionary_cost_threshold" '
    NF >= 5 {
        key = $1 "\t" $5 "\t" $2 "\t" $3
        if (!(key in cost) || $4 < cost[key]) {
            cost[key] = $4
        }
        if ($4 <= threshold) {
            eligible[$1] = 1
        }
    }
    END {
        for (key in cost) {
            split(key, columns, "\t")
            if (cost[key] <= threshold || (columns[1] in eligible && columns[1] == columns[2])) {
                print key "\t" cost[key]
            }
        }
    }
' "$dictionary_dir"/dictionary*.txt | LC_ALL=C sort > "$temporary_output"

actual_entries="$(wc -l < "$temporary_output" | tr -d ' ')"
if [[ "$actual_entries" != "$expected_entries" ]]; then
    echo "Expected $expected_entries entries, generated $actual_entries" >&2
    exit 1
fi

temporary_connection_output="$temporary_dir/mozc-connection.bin"
perl -e '
    use strict;
    use warnings;

    my ($input, $output) = @ARGV;
    open my $input_handle, "<", $input or die $!;
    my $size = <$input_handle>;
    chomp $size;

    my $entries_file = "$output.entries";
    open my $entries_handle, ">:raw", $entries_file or die $!;
    my (@offsets, @modes);
    my $entry_count = 0;
    push @offsets, 0;

    for my $right_id (0 .. $size - 1) {
        my (@row, %frequencies);
        for my $left_id (0 .. $size - 1) {
            my $line = <$input_handle>;
            defined $line or die "connection matrix ended early";
            my $cost = int($line);
            die "connection cost out of range: $cost" if $cost < 0 || $cost > 65535;
            push @row, $cost;
            $frequencies{$cost}++;
        }

        my ($mode, $highest_frequency) = (0, -1);
        for my $value (sort { $a <=> $b } keys %frequencies) {
            if ($frequencies{$value} > $highest_frequency) {
                ($mode, $highest_frequency) = ($value, $frequencies{$value});
            }
        }
        push @modes, $mode;

        for my $left_id (0 .. $#row) {
            next if $row[$left_id] == $mode;
            print {$entries_handle} pack("vv", $left_id, $row[$left_id]);
            $entry_count++;
        }
        push @offsets, $entry_count;
    }

    close $entries_handle;
    close $input_handle;

    open my $output_handle, ">:raw", $output or die $!;
    print {$output_handle} pack("a4vv", "UCN2", $size, 0);
    print {$output_handle} pack("V*", @offsets);
    print {$output_handle} pack("v*", @modes);

    open my $entries_input, "<:raw", $entries_file or die $!;
    my $buffer;
    while (read($entries_input, $buffer, 65536)) {
        print {$output_handle} $buffer;
    }
    close $entries_input;
    close $output_handle;
    unlink $entries_file;
' "$dictionary_dir/connection_single_column.txt" "$temporary_connection_output"

actual_connection_bytes="$(wc -c < "$temporary_connection_output" | tr -d ' ')"
if [[ "$actual_connection_bytes" != "$expected_connection_bytes" ]]; then
    echo "Expected $expected_connection_bytes connection bytes, generated $actual_connection_bytes" >&2
    exit 1
fi

mv "$temporary_output" "$output_file"
mv "$temporary_connection_output" "$connection_output_file"
echo "Generated $output_file ($actual_entries entries)"
echo "Generated $connection_output_file ($actual_connection_bytes bytes)"
