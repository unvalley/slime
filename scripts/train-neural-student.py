#!/usr/bin/env python3
"""Train an evaluation-only N-best candidate scorer with MLX.

The bi-encoder shares an encoder between the local context/reading prefix and
candidate surfaces. The cross-encoder jointly encodes both document sides,
reading, and one candidate. Neither architecture generates text: both score
only candidates supplied by the Rust converter. Training data and model
artifacts stay under target/evaluation and are not bundled.
"""

from __future__ import annotations

import argparse
import collections
import json
import math
import random
import statistics
import time
from pathlib import Path

import mlx.core as mx
import mlx.nn as nn
import mlx.optimizers as optim
from mlx.utils import tree_flatten


PAD = "<pad>"
UNK = "<unk>"
CLS = "<cls>"
CONTEXT = "<context>"
RIGHT = "<right>"
INPUT = "<input>"
OUTPUT = "<output>"
SPECIAL_TOKENS = [PAD, UNK, CLS, CONTEXT, RIGHT, INPUT, OUTPUT]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--train", type=Path)
    parser.add_argument("--dev", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--load", type=Path)
    parser.add_argument("--architecture", choices=("bi", "cross"), default="bi")
    parser.add_argument("--epochs", type=int, default=3)
    parser.add_argument("--batch-size", type=int, default=8)
    parser.add_argument("--dimensions", type=int, default=256)
    parser.add_argument("--layers", type=int, default=4)
    parser.add_argument("--heads", type=int, default=8)
    parser.add_argument("--mlp-dimensions", type=int, default=512)
    parser.add_argument("--max-prefix-length", type=int, default=160)
    parser.add_argument("--max-candidate-length", type=int, default=64)
    parser.add_argument("--max-vocabulary", type=int, default=8192)
    parser.add_argument("--learning-rate", type=float, default=3e-4)
    parser.add_argument("--seed", type=int, default=20260803)
    parser.add_argument("--minimum-input-characters", type=int, default=0)
    parser.add_argument("--limit", type=int)
    parser.add_argument("--dev-limit", type=int)
    return parser.parse_args()


def validate_args(args: argparse.Namespace) -> None:
    positive = {
        "epochs": args.epochs,
        "batch size": args.batch_size,
        "dimensions": args.dimensions,
        "layers": args.layers,
        "heads": args.heads,
        "MLP dimensions": args.mlp_dimensions,
        "maximum prefix length": args.max_prefix_length,
        "maximum candidate length": args.max_candidate_length,
        "maximum vocabulary": args.max_vocabulary,
        "learning rate": args.learning_rate,
    }
    for name, value in positive.items():
        if value <= 0:
            raise SystemExit(f"{name} must be positive")
    if args.minimum_input_characters < 0:
        raise SystemExit("minimum input characters must be non-negative")
    for name, value in (("limit", args.limit), ("development limit", args.dev_limit)):
        if value is not None and value <= 0:
            raise SystemExit(f"{name} must be positive")
    if args.max_vocabulary < len(SPECIAL_TOKENS):
        raise SystemExit(
            f"maximum vocabulary must be at least {len(SPECIAL_TOKENS)}"
        )
    if args.max_prefix_length < 7:
        raise SystemExit("maximum prefix length must be at least 7")
    if args.max_candidate_length < 3:
        raise SystemExit("maximum candidate length must be at least 3")
    if args.dimensions % args.heads != 0:
        raise SystemExit("dimensions must be divisible by heads")
    if args.load is not None and (args.train is not None or args.output is not None):
        raise SystemExit("evaluation with --load cannot also train or write an output")
    if args.load is None and (args.train is None or args.output is None):
        raise SystemExit("training requires --train and --output; evaluation requires --load")


def load_export(path: Path) -> dict:
    with path.open(encoding="utf-8") as handle:
        export = json.load(handle)
    if not isinstance(export.get("items"), list):
        raise ValueError(f"{path} is not an N-best export")
    return export


def build_vocabulary(items: list[dict], maximum: int) -> tuple[dict[str, int], list[str]]:
    counts: collections.Counter[str] = collections.Counter()
    for item in items:
        counts.update(item["context_text"])
        counts.update(item.get("right_context_text", ""))
        counts.update(item["input"])
        for candidate in item["candidates"]:
            counts.update(candidate["surface"])
    vocabulary = SPECIAL_TOKENS + [
        character
        for character, _ in counts.most_common(maximum - len(SPECIAL_TOKENS))
    ]
    return {token: index for index, token in enumerate(vocabulary)}, vocabulary


def pad(tokens: list[str], token_ids: dict[str, int], maximum: int) -> list[int]:
    encoded = [token_ids.get(token, token_ids[UNK]) for token in tokens[:maximum]]
    return encoded + [token_ids[PAD]] * (maximum - len(encoded))


def encode_prefix(item: dict, token_ids: dict[str, int], maximum: int) -> list[int]:
    context = list(item["context_text"][-40:])
    has_right_marker = RIGHT in token_ids
    right_context = (
        list(item.get("right_context_text", "")[:40]) if has_right_marker else []
    )
    reading = list(item["input"])
    fixed = 3 + len(reading) + int(has_right_marker)
    while fixed + len(context) + len(right_context) > maximum:
        if len(context) >= len(right_context) and context:
            context.pop(0)
        elif right_context:
            right_context.pop()
        else:
            break
    if fixed > maximum:
        marker_count = 4 if has_right_marker else 3
        reading = reading[: max(0, maximum - marker_count)]
        context = []
        right_context = []
    right = [RIGHT, *right_context] if has_right_marker else []
    return pad(
        [CLS, CONTEXT, *context, *right, INPUT, *reading], token_ids, maximum
    )


def encode_candidate(surface: str, token_ids: dict[str, int], maximum: int) -> list[int]:
    return pad([CLS, OUTPUT, *surface], token_ids, maximum)


def encode_cross(
    item: dict, surface: str, token_ids: dict[str, int], maximum: int
) -> list[int]:
    context = list(item["context_text"][-40:])
    right_context = list(item.get("right_context_text", "")[:40])
    reading = list(item["input"])
    candidate = list(surface)
    fixed = 5 + len(reading) + len(candidate)
    while fixed + len(context) + len(right_context) > maximum:
        if len(context) >= len(right_context) and context:
            context.pop(0)
        elif right_context:
            right_context.pop()
        else:
            break
    if fixed > maximum:
        reading = reading[: max(0, maximum - 5)]
        candidate = candidate[: max(0, maximum - 5 - len(reading))]
    return pad(
        [
            CLS,
            CONTEXT,
            *context,
            RIGHT,
            *right_context,
            INPUT,
            *reading,
            OUTPUT,
            *candidate,
        ],
        token_ids,
        maximum,
    )


class PreparedItem:
    def __init__(
        self,
        item: dict,
        token_ids: dict[str, int],
        max_prefix_length: int,
        max_candidate_length: int,
        architecture: str,
    ):
        self.input_characters = item.get("input_characters", len(item["input"]))
        self.prefix = encode_prefix(item, token_ids, max_prefix_length)
        if architecture == "cross":
            self.sequences = [
                encode_cross(item, candidate["surface"], token_ids, max_prefix_length)
                for candidate in item["candidates"]
            ]
        else:
            self.sequences = [
                encode_candidate(candidate["surface"], token_ids, max_candidate_length)
                for candidate in item["candidates"]
            ]
        self.base_scores = [-candidate["cost"] / 500.0 for candidate in item["candidates"]]
        self.label = item["label_index"]


def prepare(
    items: list[dict],
    token_ids: dict[str, int],
    max_prefix_length: int,
    max_candidate_length: int,
    architecture: str,
) -> list[PreparedItem]:
    return [
        PreparedItem(
            item,
            token_ids,
            max_prefix_length,
            max_candidate_length,
            architecture,
        )
        for item in items
    ]


def make_batch(
    items: list[PreparedItem],
    indices: list[int],
    max_candidate_length: int,
):
    candidate_count = max(len(items[index].sequences) for index in indices)
    prefixes = []
    candidates = []
    base_scores = []
    valid = []
    labels = []
    padding = [0] * max_candidate_length
    for index in indices:
        item = items[index]
        missing = candidate_count - len(item.sequences)
        prefixes.append(item.prefix)
        candidates.append(item.sequences + [padding] * missing)
        base_scores.append(item.base_scores + [0.0] * missing)
        valid.append([True] * len(item.sequences) + [False] * missing)
        labels.append(item.label if item.label is not None else 0)
    return (
        mx.array(prefixes, dtype=mx.int32),
        mx.array(candidates, dtype=mx.int32),
        mx.array(base_scores, dtype=mx.float32),
        mx.array(valid, dtype=mx.bool_),
        mx.array(labels, dtype=mx.int32),
    )


class CandidateScorer(nn.Module):
    def __init__(
        self,
        vocabulary_size: int,
        dimensions: int,
        layers: int,
        heads: int,
        mlp_dimensions: int,
        max_length: int,
    ):
        super().__init__()
        self.token_embedding = nn.Embedding(vocabulary_size, dimensions)
        self.position_embedding = nn.Embedding(max_length, dimensions)
        self.encoder = nn.TransformerEncoder(
            layers,
            dimensions,
            heads,
            mlp_dims=mlp_dimensions,
            dropout=0.0,
            norm_first=True,
        )
        self.match = nn.Sequential(
            nn.Linear(dimensions * 4, dimensions),
            nn.GELU(),
            nn.Linear(dimensions, 1),
        )

    def encode(self, tokens: mx.array) -> mx.array:
        length = tokens.shape[-1]
        positions = mx.arange(length)
        hidden = self.token_embedding(tokens) + self.position_embedding(positions)
        key_mask = tokens != 0
        attention_mask = mx.where(key_mask[:, None, None, :], 0.0, -1e9)
        encoded = self.encoder(hidden, attention_mask)
        return encoded[:, 0, :]

    def __call__(self, prefixes: mx.array, candidates: mx.array) -> mx.array:
        batch, candidate_count, length = candidates.shape
        prefix = self.encode(prefixes)
        candidate = self.encode(candidates.reshape(batch * candidate_count, length)).reshape(
            batch, candidate_count, -1
        )
        prefix = mx.broadcast_to(prefix[:, None, :], candidate.shape)
        features = mx.concatenate(
            [prefix, candidate, prefix * candidate, mx.abs(prefix - candidate)], axis=-1
        )
        return self.match(features).squeeze(-1)


class CrossCandidateScorer(nn.Module):
    def __init__(
        self,
        vocabulary_size: int,
        dimensions: int,
        layers: int,
        heads: int,
        mlp_dimensions: int,
        max_length: int,
    ):
        super().__init__()
        self.token_embedding = nn.Embedding(vocabulary_size, dimensions)
        self.position_embedding = nn.Embedding(max_length, dimensions)
        self.encoder = nn.TransformerEncoder(
            layers,
            dimensions,
            heads,
            mlp_dims=mlp_dimensions,
            dropout=0.0,
            norm_first=True,
        )
        self.score = nn.Linear(dimensions, 1)

    def __call__(self, prefixes: mx.array, candidates: mx.array) -> mx.array:
        del prefixes
        batch, candidate_count, length = candidates.shape
        tokens = candidates.reshape(batch * candidate_count, length)
        positions = mx.arange(length)
        hidden = self.token_embedding(tokens) + self.position_embedding(positions)
        key_mask = tokens != 0
        attention_mask = mx.where(key_mask[:, None, None, :], 0.0, -1e9)
        encoded = self.encoder(hidden, attention_mask)
        return self.score(encoded[:, 0, :]).reshape(batch, candidate_count)


def loss_fn(
    model: nn.Module,
    prefixes: mx.array,
    candidates: mx.array,
    base_scores: mx.array,
    valid: mx.array,
    labels: mx.array,
) -> mx.array:
    logits = base_scores + model(prefixes, candidates)
    logits = mx.where(valid, logits, -1e9)
    return nn.losses.cross_entropy(logits, labels, reduction="mean")


def parameter_count(model: nn.Module) -> int:
    return sum(value.size for _, value in tree_flatten(model.parameters()))


def batches(count: int, batch_size: int, order: list[int]):
    for start in range(0, count, batch_size):
        yield order[start : start + batch_size]


def score_items(
    model: nn.Module,
    items: list[PreparedItem],
    batch_size: int,
    max_candidate_length: int,
) -> tuple[list[list[float]], list[list[float]]]:
    model.eval()
    model_scores: list[list[float]] = []
    base_scores: list[list[float]] = []
    order = list(range(len(items)))
    for indices in batches(len(items), batch_size, order):
        prefixes, candidates, bases, valid, _ = make_batch(
            items, indices, max_candidate_length
        )
        scores = model(prefixes, candidates)
        mx.eval(scores)
        scores_list = scores.tolist()
        for row, index in zip(scores_list, indices):
            count = len(items[index].sequences)
            model_scores.append(row[:count])
            base_scores.append(items[index].base_scores)
    return model_scores, base_scores


def evaluate(
    items: list[PreparedItem],
    model_scores: list[list[float]],
    base_scores: list[list[float]],
    minimum_input_characters: int,
) -> list[dict]:
    reports = []
    for weight in [0.0, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0]:
        correct = 0
        oracle = 0
        reciprocal_rank = 0.0
        for item, student, base in zip(items, model_scores, base_scores):
            effective_weight = (
                weight if item.input_characters >= minimum_input_characters else 0.0
            )
            ranking = sorted(
                range(len(student)),
                key=lambda index: base[index] + effective_weight * student[index],
                reverse=True,
            )
            if item.label is not None:
                oracle += 1
                rank = ranking.index(item.label)
                correct += rank == 0
                reciprocal_rank += 1.0 / (rank + 1)
        reports.append(
            {
                "weight": weight,
                "accuracy_at_1": correct / len(items),
                "accuracy_at_k": oracle / len(items),
                "mrr_at_k": reciprocal_rank / len(items),
            }
        )
    return reports


def best_weight_report(reports: list[dict]) -> dict:
    return max(
        reports,
        key=lambda report: (
            report["accuracy_at_1"],
            report["mrr_at_k"],
            -report["weight"],
        ),
    )


def benchmark(model: nn.Module, item: PreparedItem, max_candidate_length: int) -> dict:
    prefixes, candidates, _, _, _ = make_batch([item], [0], max_candidate_length)
    for _ in range(10):
        result = model(prefixes, candidates)
        mx.eval(result)
    durations = []
    for _ in range(200):
        started = time.perf_counter_ns()
        result = model(prefixes, candidates)
        mx.eval(result)
        durations.append((time.perf_counter_ns() - started) / 1_000_000)
    durations.sort()
    return {
        "p50_ms": statistics.median(durations),
        "p95_ms": durations[math.ceil(len(durations) * 0.95) - 1],
        "p99_ms": durations[math.ceil(len(durations) * 0.99) - 1],
        "max_ms": durations[-1],
        "candidate_count": len(item.sequences),
    }


def main() -> None:
    args = parse_args()
    validate_args(args)
    randomizer = random.Random(args.seed)
    mx.random.seed(args.seed)
    dev_export = load_export(args.dev)
    dev_items = dev_export["items"][: args.dev_limit]
    if not dev_items:
        raise SystemExit("development export has no items")

    if args.load is not None:
        with (args.load / "config.json").open(encoding="utf-8") as handle:
            config = json.load(handle)
        with (args.load / "vocabulary.json").open(encoding="utf-8") as handle:
            vocabulary = json.load(handle)
        token_ids = {token: index for index, token in enumerate(vocabulary)}
        architecture = config.get("architecture", "bi")
        dev = prepare(
            dev_items,
            token_ids,
            config["max_prefix_length"],
            config["max_candidate_length"],
            architecture,
        )
        model_type = CrossCandidateScorer if architecture == "cross" else CandidateScorer
        model = model_type(
            len(vocabulary),
            config["dimensions"],
            config["layers"],
            config["heads"],
            config["mlp_dimensions"],
            max(config["max_prefix_length"], config["max_candidate_length"]),
        )
        model.load_weights(str(args.load / "model.safetensors"))
        mx.eval(model.parameters())
        sequence_length = (
            config["max_prefix_length"]
            if architecture == "cross"
            else config["max_candidate_length"]
        )
        model_scores, base_scores = score_items(
            model, dev, max(args.batch_size, 16), sequence_length
        )
        print(
            json.dumps(
                {
                    "dev": evaluate(
                        dev,
                        model_scores,
                        base_scores,
                        args.minimum_input_characters,
                    ),
                    "latency": benchmark(model, dev[0], sequence_length),
                    "parameters": parameter_count(model),
                    "weights_bytes": (args.load / "model.safetensors").stat().st_size,
                },
                ensure_ascii=False,
            ),
            flush=True,
        )
        return

    assert args.train is not None
    assert args.output is not None
    train_export = load_export(args.train)
    train_items = train_export["items"][: args.limit]
    token_ids, vocabulary = build_vocabulary(train_items, args.max_vocabulary)
    train = prepare(
        train_items,
        token_ids,
        args.max_prefix_length,
        args.max_candidate_length,
        args.architecture,
    )
    dev = prepare(
        dev_items,
        token_ids,
        args.max_prefix_length,
        args.max_candidate_length,
        args.architecture,
    )
    train = [item for item in train if item.label is not None]
    if not train:
        raise SystemExit("training export has no reachable labels")
    if args.output.exists() and (
        not args.output.is_dir() or any(args.output.iterdir())
    ):
        raise SystemExit(f"output directory is not empty: {args.output}")
    args.output.mkdir(parents=True, exist_ok=True)

    model_type = CrossCandidateScorer if args.architecture == "cross" else CandidateScorer
    model = model_type(
        len(vocabulary),
        args.dimensions,
        args.layers,
        args.heads,
        args.mlp_dimensions,
        max(args.max_prefix_length, args.max_candidate_length),
    )
    optimizer = optim.AdamW(learning_rate=args.learning_rate, weight_decay=1e-4)
    loss_and_grad = nn.value_and_grad(model, loss_fn)
    parameters = parameter_count(model)
    print(
        json.dumps(
            {
                "device": str(mx.default_device()),
                "train_items": len(train),
                "dev_items": len(dev),
                "vocabulary": len(vocabulary),
                "parameters": parameters,
                "fp16_bytes": parameters * 2,
                "fp32_bytes": parameters * 4,
            },
            ensure_ascii=False,
        ),
        flush=True,
    )

    order = list(range(len(train)))
    sequence_length = (
        args.max_prefix_length
        if args.architecture == "cross"
        else args.max_candidate_length
    )
    best_key: tuple[float, float, float] | None = None
    best_epoch = 0
    best_report: dict | None = None
    for epoch in range(args.epochs):
        model.train()
        randomizer.shuffle(order)
        losses = []
        started = time.perf_counter()
        for step, indices in enumerate(batches(len(train), args.batch_size, order), 1):
            batch = make_batch(train, indices, sequence_length)
            loss, gradients = loss_and_grad(model, *batch)
            optimizer.update(model, gradients)
            mx.eval(model.parameters(), optimizer.state, loss)
            losses.append(float(loss.item()))
            if step % 100 == 0:
                print(
                    json.dumps(
                        {
                            "epoch": epoch + 1,
                            "step": step,
                            "mean_loss": statistics.fmean(losses[-100:]),
                        }
                    ),
                    flush=True,
                )
        model_scores, base_scores = score_items(
            model, dev, max(args.batch_size, 16), sequence_length
        )
        reports = evaluate(
            dev,
            model_scores,
            base_scores,
            args.minimum_input_characters,
        )
        selected = best_weight_report(reports)
        selected_key = (
            selected["accuracy_at_1"],
            selected["mrr_at_k"],
            -selected["weight"],
        )
        if best_key is None or selected_key > best_key:
            best_key = selected_key
            best_epoch = epoch + 1
            best_report = selected
            model.save_weights(str(args.output / "model.safetensors"))
        print(
            json.dumps(
                {
                    "epoch": epoch + 1,
                    "seconds": time.perf_counter() - started,
                    "mean_loss": statistics.fmean(losses),
                    "dev": reports,
                },
                ensure_ascii=False,
            ),
            flush=True,
        )

    assert best_report is not None
    model.load_weights(str(args.output / "model.safetensors"))
    mx.eval(model.parameters())
    with (args.output / "vocabulary.json").open("w", encoding="utf-8") as handle:
        json.dump(vocabulary, handle, ensure_ascii=False)
    with (args.output / "config.json").open("w", encoding="utf-8") as handle:
        json.dump(
            vars(args)
            | {
                "parameters": parameters,
                "best_epoch": best_epoch,
                "best_weight": best_report["weight"],
            },
            handle,
            ensure_ascii=False,
            default=str,
        )
    model_scores, base_scores = score_items(
        model, dev, max(args.batch_size, 16), sequence_length
    )
    final_report = {
        "dev": evaluate(
            dev,
            model_scores,
            base_scores,
            args.minimum_input_characters,
        ),
        "latency": benchmark(model, dev[0], sequence_length),
        "parameters": parameters,
        "weights_bytes": (args.output / "model.safetensors").stat().st_size,
        "best_epoch": best_epoch,
        "best_weight": best_report["weight"],
    }
    print(json.dumps(final_report, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
