# SPDX-License-Identifier: LGPL-3.0-or-later
# SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
"""Motion graphics of the tour (Manim Community 0.19+), in the Nord Frost colours of the docs.

    manim render -qh --fps 30 scenes.py Intro Problem How Record Outro
"""
from manim import (
    DOWN, LEFT, RIGHT, UP, UL, Arrow, Create, Dot, FadeIn, FadeOut, Flash, GrowArrow, Indicate,
    LaggedStart, Line, MoveAlongPath, Rectangle, RoundedRectangle, Scene, Text, VGroup, Write,
    config, linear,
)

BG, PANEL, SEL = "#2E3440", "#3B4252", "#434C5E"
TEXT, DIM = "#ECEFF4", "#7B88A1"
FROST, FROST2, FROST3 = "#88C0D0", "#81A1C1", "#5E81AC"
GREEN, YELLOW, ORANGE, RED = "#A3BE8C", "#EBCB8B", "#D08770", "#BF616A"
SANS, MONO = "Inter", "JetBrains Mono"

config.background_color = BG


def sans(s, size=40, color=TEXT, weight="MEDIUM"):
    return Text(s, font=SANS, font_size=size, color=color, weight=weight)


def mono(s, size=32, color=TEXT, weight="NORMAL"):
    return Text(s, font=MONO, font_size=size, color=color, weight=weight)


def box(w, h, color=FROST3, fill=PANEL):
    return RoundedRectangle(width=w, height=h, corner_radius=0.18, stroke_color=color, stroke_width=3,
                            fill_color=fill, fill_opacity=1)


def node(name, load, w=2.6, h=2.2):
    """A compute node: a box, its name and a CPU bar filled to `load` (0..1)."""
    b = box(w, h)
    label = mono(name, 28, FROST).next_to(b.get_top(), DOWN, buff=0.25)
    track = Rectangle(width=w - 0.6, height=0.32, stroke_width=0, fill_color=SEL, fill_opacity=1)
    track.next_to(label, DOWN, buff=0.45)
    colour = GREEN if load > 0.5 else RED
    bar = Rectangle(width=(w - 0.6) * load, height=0.32, stroke_width=0, fill_color=colour, fill_opacity=1)
    bar.align_to(track, LEFT).align_to(track, DOWN)
    pct = mono(f"{round(load * 100)} % CPU", 22, DIM).next_to(track, DOWN, buff=0.2)
    return VGroup(b, label, track, bar, pct)


class Intro(Scene):
    def construct(self):
        name = mono("unjam", 140, FROST, "BOLD")
        rule = Line(LEFT * 3.2, RIGHT * 3.2, color=FROST3, stroke_width=4).next_to(name, DOWN, buff=0.35)
        sub = sans("See inside your Slurm job. Live, on every node.", 44).next_to(rule, DOWN, buff=0.5)
        self.play(Write(name), run_time=1.2)
        self.play(Create(rule), FadeIn(sub, shift=UP * 0.2), run_time=0.9)
        self.wait(2.2)
        self.play(FadeOut(VGroup(name, rule, sub)), run_time=0.6)


class Problem(Scene):
    def construct(self):
        loads = [0.9, 0.89, 0.14, 0.88]
        nodes = VGroup(*[node(f"node{i + 1}", v, 3.0, 2.5) for i, v in enumerate(loads)]).arrange(RIGHT, buff=0.45)
        nodes.shift(DOWN * 1.3)
        q1 = sans("Your 4-node job is slow.", 56).to_edge(UP, buff=1.0)
        q2 = sans("Which node? Which process?", 44, YELLOW).next_to(q1, DOWN, buff=0.3)
        self.play(FadeIn(q1, shift=DOWN * 0.2), run_time=0.6)
        self.play(LaggedStart(*[FadeIn(n, shift=UP * 0.3) for n in nodes], lag_ratio=0.15), run_time=1.2)
        self.play(FadeIn(q2), run_time=0.5)
        self.wait(1.0)
        slow = nodes[2][0]
        self.play(slow.animate.set_stroke(RED, width=6), Indicate(nodes[2][3], color=RED, scale_factor=1.3),
                  run_time=1.0)
        self.play(Flash(slow.get_corner(UL + RIGHT * 0), color=RED, flash_radius=0.4), run_time=0.6)
        self.wait(1.5)
        self.play(FadeOut(VGroup(nodes, q1, q2)), run_time=0.6)


class How(Scene):
    def construct(self):
        login = box(4.4, 1.3, FROST)
        cmd = mono("$ unjam --slurm", 28, FROST).move_to(login)
        login_g = VGroup(login, cmd).to_edge(LEFT, buff=0.7)
        nodes, agents = VGroup(), VGroup()
        for i in range(4):
            b = box(4.2, 1.0)
            t = mono(f"node{i + 1}", 26, TEXT).move_to(b).shift(LEFT * 1.2)
            chip = RoundedRectangle(width=1.5, height=0.55, corner_radius=0.12, stroke_width=0,
                                    fill_color=FROST3, fill_opacity=1).move_to(b).shift(RIGHT * 0.95)
            ct = mono("agent", 22, TEXT, "BOLD").move_to(chip)
            nodes.add(VGroup(b, t))
            agents.add(VGroup(chip, ct))
        col = VGroup(*[VGroup(n, a) for n, a in zip(nodes, agents)]).arrange(DOWN, buff=0.22)
        col.to_edge(RIGHT, buff=0.7).shift(DOWN * 0.35)
        login_g.match_y(col)
        arrows = VGroup(*[Arrow(login.get_right(), n[0].get_left(), buff=0.15, color=FROST2, stroke_width=4,
                                max_tip_length_to_length_ratio=0.06) for n in nodes])
        srun = mono("srun --overlap", 24, DIM).move_to(arrows).shift(UP * 0.3 + LEFT * 0.4)
        srun.add_background_rectangle(color=BG, opacity=1, buff=0.08)
        title = sans("One small agent per node", 46).to_edge(UP, buff=0.45)
        self.play(FadeIn(login_g, shift=RIGHT * 0.3), run_time=0.7)
        self.play(LaggedStart(*[GrowArrow(a) for a in arrows], lag_ratio=0.15), FadeIn(srun),
                  FadeIn(nodes), run_time=1.2)
        self.play(LaggedStart(*[FadeIn(a, scale=0.6) for a in agents], lag_ratio=0.15), FadeIn(title),
                  run_time=0.9)
        # The agents read the cgroup v2 tree and stream samples back.
        cap2 = sans("reads the cgroup v2 tree and streams it back", 32, DIM).next_to(title, DOWN, buff=0.15)
        self.play(FadeIn(cap2), run_time=0.5)
        for _ in range(3):
            dots = [Dot(color=YELLOW if k != 2 else RED, radius=0.09) for k in range(4)]
            paths = [Line(n[0].get_left(), login.get_right()) for n in nodes]
            self.play(*[MoveAlongPath(d, p, rate_func=linear) for d, p in zip(dots, paths)], run_time=0.7)
            self.remove(*dots)
        tags = VGroup(*[sans(s, 30, GREEN) for s in ("no daemon", "no root", "one static binary")])
        tags.arrange(RIGHT, buff=0.9).to_edge(DOWN, buff=0.5)
        self.play(LaggedStart(*[FadeIn(t, shift=UP * 0.2) for t in tags], lag_ratio=0.25), run_time=1.0)
        self.wait(1.6)
        self.play(FadeOut(VGroup(login_g, col, arrows, srun, title, cap2, tags)), run_time=0.6)


class Record(Scene):
    def construct(self):
        title = sans("Record a job step. Replay it later.", 50).to_edge(UP, buff=0.6)
        cmd_box = box(7.6, 1.0, FROST)
        cmd = mono("$ srun unjam record -- ./app", 28, FROST).move_to(cmd_box)
        src = VGroup(cmd_box, cmd).shift(UP * 1.4)
        files = VGroup()
        for i in range(4):
            f = RoundedRectangle(width=2.7, height=0.8, corner_radius=0.1, stroke_color=FROST3, stroke_width=2,
                                 fill_color=PANEL, fill_opacity=1)
            files.add(VGroup(f, mono(f"node{i + 1}.arrows", 22, TEXT).move_to(f)))
        files.arrange(RIGHT, buff=0.35).shift(DOWN * 0.3)
        fan = VGroup(*[Arrow(cmd_box.get_bottom(), f.get_top(), buff=0.1, color=FROST2, stroke_width=3,
                             max_tip_length_to_length_ratio=0.12) for f in files])
        outs = VGroup(mono("$ unjam --replay *.arrows", 28, GREEN), mono("pyarrow  →  Parquet", 28, YELLOW))
        outs.arrange(RIGHT, buff=1.4).shift(DOWN * 2.2)
        note = sans("Arrow IPC, one file per node, every batch readable on its own", 28, DIM).to_edge(DOWN, buff=0.35)
        self.play(FadeIn(title), FadeIn(src, shift=DOWN * 0.2), run_time=0.8)
        self.play(LaggedStart(*[GrowArrow(a) for a in fan], lag_ratio=0.1),
                  LaggedStart(*[FadeIn(f, shift=DOWN * 0.2) for f in files], lag_ratio=0.15), run_time=1.3)
        self.play(FadeIn(outs[0], shift=UP * 0.2), run_time=0.6)
        self.play(FadeIn(outs[1], shift=UP * 0.2), FadeIn(note), run_time=0.6)
        self.wait(2.6)
        self.play(FadeOut(VGroup(title, src, files, fan, outs, note)), run_time=0.6)


class Outro(Scene):
    def construct(self):
        name = mono("unjam", 110, FROST, "BOLD").to_edge(UP, buff=0.9)
        lines = VGroup(
            VGroup(sans("mise", 30, DIM), mono("$ mise use -g github:GSI-HPC/unjam", 32)),
            VGroup(sans("Rocky 9 · 10, Fedora", 30, DIM), mono("$ dnf install unjam", 32)),
        )
        repo = sans("dnf: add the repository first, see the docs", 24, DIM)
        lines[1][1].next_to(lines[0][1], DOWN, buff=0.45, aligned_edge=LEFT)
        for label, cmd in lines:
            label.next_to(cmd, LEFT, buff=0.5)
        lines.next_to(name, DOWN, buff=0.8)
        repo.next_to(lines, DOWN, buff=0.25).align_to(lines[1][1], LEFT)
        url = mono("gsi-hpc.github.io/unjam", 40, YELLOW, "BOLD").next_to(repo, DOWN, buff=0.7)
        gh = sans("github.com/GSI-HPC/unjam  ·  LGPL-3.0", 28, DIM).next_to(url, DOWN, buff=0.3)
        url.set_x(0)
        gh.set_x(0)
        self.play(Write(name), run_time=1.0)
        self.play(LaggedStart(*[FadeIn(g, shift=RIGHT * 0.3) for g in lines], lag_ratio=0.3), run_time=1.2)
        self.play(FadeIn(repo), FadeIn(url, scale=0.9), FadeIn(gh), run_time=0.8)
        self.wait(4.5)
        self.play(FadeOut(VGroup(name, lines, repo, url, gh)), run_time=0.8)
