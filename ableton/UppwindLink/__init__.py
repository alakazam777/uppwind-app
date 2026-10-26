# -*- coding: utf-8 -*-
# SPDX-License-Identifier: MIT
# Copyright (C) 2026 Uppwind
"""Uppwind Link -- Remote Script Ableton Live 11 / 12 (essaye dans Live 12.4.6 le 14 oct. 2026 : see ../README.md).

Dossier a copier dans  <User Library>/Remote Scripts/UppwindLink/  (puis redemarrer Live)
puis Settings > Tempo & MIDI (Live 11 : Preferences > Link, Tempo & MIDI) > Control Surface = UppwindLink.
"""


def create_instance(c_instance):
    # Import tardif : link_core se teste hors de Live, sans _Framework
    from .UppwindLink import UppwindLinkSurface
    return UppwindLinkSurface(c_instance)
