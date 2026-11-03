
/* WARNING: Globals starting with '_' overlap smaller symbols at the same address */

void FUN_0290dca0(long param_1,long param_2,long *param_3,undefined8 param_4)

{
  undefined4 *puVar1;
  undefined1 uVar2;
  undefined4 uVar3;
  void *pvVar4;
  undefined8 *puVar5;
  void *__dest;
  void *pvVar6;
  long *plVar7;
  long *plVar8;
  undefined8 *puVar9;
  long lVar10;
  undefined4 *puVar11;
  undefined8 *puVar12;
  undefined1 *puVar13;
  ulong uVar14;
  undefined8 *puVar15;
  long lVar16;
  undefined4 uVar17;
  undefined8 uVar18;
  undefined4 local_bd8;
  undefined4 local_bd4;
  undefined8 local_bd0;
  undefined8 local_bc8;
  undefined4 local_bc0;
  long *local_bb8 [4];
  undefined1 auStack_b98 [2864];
  long local_68;
  
  local_68 = ___stack_chk_guard;
  *(long **)(param_1 + 0x2310) = param_3;
  if (param_2 == 0) {
    local_bd0 = 3;
    local_bc8 = 0x100000000;
    local_bc0 = 0x3f800000;
    FUN_02874a40(local_bb8,0,0);
    lVar10 = param_1 + 0xcf8;
    FUN_0286dcb0(param_1);
    FUN_02879b90(param_1 + 0xb38);
    do {
      FUN_014b16a0(lVar10,0);
      lVar16 = lVar10 + 0x160;
      lVar10 = lVar10 + 0x58;
      FUN_014b16a0(lVar16,0);
    } while (lVar10 != param_1 + 0xe58);
    FUN_014af150(param_1 + 0xcd8,0);
    FUN_014b16a0(param_1 + 0x1538,0);
    FUN_01db15f0(param_1 + 0x1590,1);
    FUN_0160b380(param_1 + 0x15e8,0);
    FUN_014b3740(param_1 + 0x1640,1);
    FUN_014904f0(param_1 + 0x1698,0);
    local_bd8 = 0;
    if (*(long *)(param_1 + 0x1758) != 0) {
      uVar3 = (**(code **)(param_1 + 0x1760))(param_1 + 0x1748,&local_bd8);
      FUN_014b16a0(param_1 + 0x16f0,uVar3);
      local_bd4 = 0x7f;
      if (*(long *)(param_1 + 0x17d0) != 0) {
        uVar3 = (**(code **)(param_1 + 0x17d8))(param_1 + 0x17c0,&local_bd4);
        FUN_014b16a0(param_1 + 0x1768,uVar3);
        *(undefined8 *)(param_1 + 0x2460) = 0;
        FUN_014b16a0(param_1 + 0x1f18,0xe);
        FUN_014904f0(param_1 + 0x1fc8,0);
        FUN_014904f0(param_1 + 0x2020,1);
        local_bd0 = CONCAT44(local_bd0._4_4_,0x42f00000);
        if (*(long *)(param_1 + 0x20e0) != 0) {
          (**(code **)(param_1 + 0x20e8))(param_1 + 0x20d0,&local_bd0);
          FUN_014bb930(param_1 + 0x2078);
          local_bb8[0] = (long *)CONCAT44(local_bb8[0]._4_4_,100);
          if (*(long *)(param_1 + 0x2158) != 0) {
            uVar3 = (**(code **)(param_1 + 0x2160))(param_1 + 0x2148,local_bb8);
            FUN_014b3740(param_1 + 0x20f0,uVar3);
            FUN_015916f0(param_1 + 0x21e0,1);
            lVar10 = 0;
            FUN_014b16a0(param_1 + 0x2238,0);
            FUN_014b16a0(param_1 + 0x2290,0);
            do {
              lVar16 = *(long *)(param_1 + 0x22e8);
              FUN_02919ed0(auStack_b98,*(undefined8 *)(param_1 + 0x648),
                           *(undefined8 *)(param_1 + 0x2300));
                    /* try { // try from 0290e458 to 0290e45b has its CatchHandler @ 0290eda8 */
              FUN_02918be0(lVar16 + lVar10,auStack_b98);
              lVar10 = lVar10 + 0xb30;
              FUN_02914410(auStack_b98);
            } while (lVar10 != 0x5980);
            *(undefined4 *)(param_1 + 0x2348) = 0x3f000000;
            *(undefined2 *)(param_1 + 0x234c) = 0;
            *(undefined1 *)(param_1 + 0x234e) = 0;
            *(undefined4 *)(param_1 + 0x2350) = 0;
            *(undefined2 *)(param_1 + 0x2354) = 0x100;
            *(undefined4 *)(param_1 + 0x2358) = 0;
            *(undefined1 *)(param_1 + 0x235c) = 0;
            FUN_014904f0(param_1 + 0x1f70,1);
            if (*(long *)(param_1 + 0x2458) == 0) {
              puVar12 = operator_new(0x270);
              *puVar12 = &PTR_FUN_06e8ca20;
                    /* try { // try from 0290e844 to 0290e847 has its CatchHandler @ 0290ee0c */
              FUN_0246f0d0(puVar12 + 1);
              puVar12[5] = 0;
              puVar12[6] = 0;
              puVar12[7] = 0;
              puVar12[8] = 0;
              *(undefined4 *)(puVar12 + 7) = 1;
              puVar12[9] = 0;
              puVar12[10] = 0;
              *(undefined4 *)(puVar12 + 0xb) = 0;
                    /* try { // try from 0290e870 to 0290e873 has its CatchHandler @ 0290efc0 */
              FUN_0246f0d0(puVar12 + 0xc);
              puVar12[0x10] = 0;
              puVar12[0x11] = 0;
              puVar12[0x12] = 0;
              puVar12[0x13] = 0;
              *(undefined4 *)(puVar12 + 0x12) = 1;
              puVar12[0x14] = 0;
              puVar12[0x15] = 0;
              *(undefined4 *)(puVar12 + 0x16) = 0;
                    /* try { // try from 0290e89c to 0290e89f has its CatchHandler @ 0290efb4 */
              FUN_0246f0d0(puVar12 + 0x17);
              puVar12[0x1b] = 0;
              puVar12[0x1c] = 0;
              puVar12[0x1d] = 0;
              puVar12[0x1e] = 0;
              *(undefined4 *)(puVar12 + 0x1d) = 1;
              puVar12[0x1f] = 0;
              puVar12[0x20] = 0;
              *(undefined4 *)(puVar12 + 0x21) = 0;
                    /* try { // try from 0290e8c4 to 0290e8c7 has its CatchHandler @ 0290efa8 */
              FUN_0246f0d0(puVar12 + 0x22);
              puVar12[0x26] = 0;
              puVar12[0x27] = 0;
              puVar12[0x28] = 0;
              puVar12[0x29] = 0;
              *(undefined4 *)(puVar12 + 0x28) = 1;
              puVar12[0x2a] = 0;
              puVar12[0x2b] = 0;
              *(undefined4 *)(puVar12 + 0x2c) = 0;
                    /* try { // try from 0290e8ec to 0290e8ef has its CatchHandler @ 0290ef9c */
              FUN_0246f0d0(puVar12 + 0x2d);
              puVar12[0x33] = 0;
              puVar12[0x34] = 0;
              puVar12[0x31] = 0;
              puVar12[0x32] = 0;
              *(undefined4 *)(puVar12 + 0x33) = 1;
              puVar12[0x35] = 0;
              puVar12[0x36] = 0;
              *(undefined4 *)(puVar12 + 0x37) = 0;
                    /* try { // try from 0290e914 to 0290e917 has its CatchHandler @ 0290ef90 */
              FUN_0246f0d0(puVar12 + 0x38);
              puVar12[0x3c] = 0;
              puVar12[0x3d] = 0;
              puVar12[0x3e] = 0;
              puVar12[0x3f] = 0;
              *(undefined4 *)(puVar12 + 0x3e) = 1;
              puVar12[0x40] = 0;
              puVar12[0x41] = 0;
              *(undefined4 *)(puVar12 + 0x42) = 0;
                    /* try { // try from 0290e93c to 0290e93f has its CatchHandler @ 0290ef84 */
              FUN_0246f0d0(puVar12 + 0x43);
              puVar12[0x49] = 0;
              puVar12[0x4a] = 0;
              puVar12[0x47] = 0;
              puVar12[0x48] = 0;
              *(undefined4 *)(puVar12 + 0x49) = 1;
              puVar12[0x4b] = 0;
              puVar12[0x4c] = 0;
              *(undefined4 *)(puVar12 + 0x4d) = 0;
                    /* try { // try from 0290e960 to 0290e963 has its CatchHandler @ 0290ef40 */
              FUN_02190a60(puVar12);
              plVar7 = *(long **)(param_1 + 0x2458);
              *(undefined8 **)(param_1 + 0x2458) = puVar12;
              if (plVar7 != (long *)0x0) {
                if (*(code **)(*plVar7 + 8) == (code *)&LAB_015f2770) {
                  *plVar7 = (long)&PTR_FUN_06e8ca20;
                  FUN_0246f110(plVar7 + 0x43);
                  FUN_0246f110(plVar7 + 0x38);
                  FUN_0246f110(plVar7 + 0x2d);
                  FUN_0246f110(plVar7 + 0x22);
                  FUN_0246f110(plVar7 + 0x17);
                  FUN_0246f110(plVar7 + 0xc);
                  FUN_0246f110(plVar7 + 1);
                  operator_delete(plVar7,0x270);
                }
                else {
                  (**(code **)(*plVar7 + 8))(plVar7);
                }
              }
            }
            else {
              FUN_02190a60();
            }
            if (param_3 == (long *)0x0) {
              plVar7 = operator_new(0x3000);
                    /* try { // try from 0290ebcc to 0290ebcf has its CatchHandler @ 0290edd8 */
              FUN_02921b80(plVar7,0);
              local_bb8[0] = plVar7;
            }
            else {
              (**(code **)(*param_3 + 0x1b8))(local_bb8,param_3);
            }
            plVar7 = local_bb8[0];
            if (*(long *)(param_1 + 0x2340) == 0) {
              local_bb8[0] = (long *)0x0;
              *(long **)(param_1 + 0x2340) = plVar7;
            }
            else {
                    /* try { // try from 0290e4d8 to 0290e4ef has its CatchHandler @ 0290ed68 */
              FUN_02922260(*(long *)(param_1 + 0x2340),local_bb8[0]);
            }
            FUN_02576d50(auStack_b98,*(undefined8 *)(param_1 + 0x648),&DAT_04bd9480);
            lVar16 = param_1 + 0x2368;
                    /* try { // try from 0290e500 to 0290e503 has its CatchHandler @ 0290ef2c */
            FUN_02576c30(lVar16,auStack_b98);
            lVar10 = param_1 + 0x1278;
            uVar14 = 0;
            FUN_02576480(auStack_b98);
            do {
                    /* try { // try from 0290e520 to 0290e543 has its CatchHandler @ 0290ed68 */
              uVar3 = FUN_02577080(lVar16,uVar14 & 0xffffffff);
              FUN_014b16a0(lVar10 + -0x2c0,uVar3);
              FUN_025778b0(lVar16,uVar14 & 0xffffffff);
              FUN_014bb930(lVar10);
              plVar7 = local_bb8[0];
              uVar14 = uVar14 + 1;
              lVar10 = lVar10 + 0x58;
            } while (uVar14 != 8);
            if (local_bb8[0] != (long *)0x0) {
              if (*(code **)(*local_bb8[0] + 8) == FUN_029123e0) {
                FUN_02912950();
                operator_delete(plVar7,0x3000);
              }
              else {
                (**(code **)(*local_bb8[0] + 8))(local_bb8[0]);
              }
            }
            lVar10 = param_1 + 0x1838;
            FUN_014904f0(param_1 + 0x17e0,0);
            do {
              FUN_02913b80(lVar10,0);
              FUN_014bb930(0x3f000000,lVar10 + 0x160);
              FUN_014bb930(0x3f000000,lVar10 + 0x2c0);
              FUN_014bb930(0x3f800000,lVar10 + 0x420);
              lVar16 = lVar10 + 0x580;
              lVar10 = lVar10 + 0x58;
              FUN_02913d40(lVar16,1);
            } while (param_1 + 0x1998 != lVar10);
            if ((param_3 == (long *)0x0) || (*(int *)((long)param_3 + 0x1234) != 0)) {
              pvVar4 = *(void **)(param_1 + 0x2448);
              *(undefined8 *)(param_1 + 0x2448) = 0;
              if (pvVar4 != (void *)0x0) {
                operator_delete(pvVar4,0x680);
              }
              puVar12 = *(undefined8 **)(param_1 + 0x2450);
              *(undefined8 *)(param_1 + 0x2450) = 0;
              if (puVar12 != (undefined8 *)0x0) {
                puVar5 = puVar12 + 0x1c;
                do {
                  puVar15 = puVar5 + -0xe;
                  if (*(code **)*puVar15 == FUN_01cfe380) {
                    *puVar15 = &PTR_FUN_06e9b878;
                    FUN_0246f110(puVar5 + -0xc);
                    plVar7 = (long *)puVar5[-0xd];
                    while (plVar7 != (long *)0x0) {
                      while( true ) {
                        plVar8 = (long *)plVar7[1];
                        puVar9 = (undefined8 *)*plVar7;
                        *puVar9 = plVar8;
                        if (plVar8 == (long *)0x0) break;
                        lVar10 = plVar7[-1];
                        *plVar8 = (long)puVar9;
                        *plVar7 = (long)(plVar7 + 1);
                        plVar7[1] = (long)plVar7;
                        (**(code **)(lVar10 + 0x10))();
                        plVar7 = (long *)puVar5[-0xd];
                        if (plVar7 == (long *)0x0) goto LAB_0290e68c;
                      }
                      *plVar7 = (long)(plVar7 + 1);
                      plVar7[1] = (long)plVar7;
                      (**(code **)(plVar7[-1] + 0x10))(plVar7 + -1);
                      plVar7 = (long *)puVar5[-0xd];
                    }
                  }
                  else {
                    (**(code **)*puVar15)();
                  }
LAB_0290e68c:
                  puVar5 = puVar15;
                } while (puVar12 != puVar15);
                operator_delete(puVar12,0xe0);
              }
            }
            else {
              pvVar4 = operator_new(0x680);
              memset(pvVar4,0,0x680);
              uVar18 = NEON_fmov(0x3f800000,4);
              *(undefined8 *)((long)pvVar4 + 0x1c) = uVar18;
              *(undefined8 *)((long)pvVar4 + 0x28) = uVar18;
              puVar12 = (undefined8 *)((long)pvVar4 + 0x34);
              do {
                *(undefined4 *)((long)puVar12 + 0x14) = 0;
                puVar5 = (undefined8 *)((long)puVar12 + 0x34);
                *(undefined4 *)((long)puVar12 + 0x24) = 0;
                *(undefined1 *)(puVar12 + 6) = 0;
                *puVar12 = 0;
                *(undefined4 *)(puVar12 + 1) = 0;
                *(undefined8 *)((long)puVar12 + 0xc) = 0;
                puVar12[3] = 0x3f80000000000000;
                *(int *)(puVar12 + 4) = (int)uVar18;
                puVar12[5] = uVar18;
                puVar12 = puVar5;
              } while ((undefined8 *)((long)pvVar4 + 0x680) != puVar5);
              pvVar6 = *(void **)(param_1 + 0x2448);
              *(void **)(param_1 + 0x2448) = pvVar4;
              if (pvVar6 != (void *)0x0) {
                operator_delete(pvVar6,0x680);
              }
              puVar5 = operator_new(0xe0);
              puVar5[1] = 0;
              *puVar5 = 0;
              puVar5[3] = 0;
              puVar5[2] = 0;
              puVar5[5] = 0;
              puVar5[4] = 0;
              puVar5[7] = 0;
              puVar5[6] = 0;
              puVar5[9] = 0;
              puVar5[8] = 0;
              puVar5[0xb] = 0;
              puVar5[10] = 0;
              puVar5[0xd] = 0;
              puVar5[0xc] = 0;
              puVar5[0xf] = 0;
              puVar5[0xe] = 0;
              puVar5[0x11] = 0;
              puVar5[0x10] = 0;
              puVar5[0x13] = 0;
              puVar5[0x12] = 0;
              puVar5[0x15] = 0;
              puVar5[0x14] = 0;
              puVar5[0x17] = 0;
              puVar5[0x16] = 0;
              puVar5[0x19] = 0;
              puVar5[0x18] = 0;
              puVar5[0x1b] = 0;
              puVar5[0x1a] = 0;
                    /* try { // try from 0290ead8 to 0290eadb has its CatchHandler @ 0290eed4 */
              FUN_01cfe150();
              puVar12 = puVar5 + 0xe;
                    /* try { // try from 0290eae4 to 0290eae7 has its CatchHandler @ 0290eea0 */
              FUN_01cfe150(puVar12);
              puVar15 = *(undefined8 **)(param_1 + 0x2450);
              *(undefined8 **)(param_1 + 0x2450) = puVar5;
              if (puVar15 != (undefined8 *)0x0) {
                puVar12 = puVar15 + 0x1c;
                do {
                  puVar5 = puVar12 + -0xe;
                  if (*(code **)*puVar5 == FUN_01cfe380) {
                    *puVar5 = &PTR_FUN_06e9b878;
                    FUN_0246f110(puVar12 + -0xc);
                    plVar7 = (long *)puVar12[-0xd];
                    while (plVar7 != (long *)0x0) {
                      while( true ) {
                        plVar8 = (long *)plVar7[1];
                        puVar9 = (undefined8 *)*plVar7;
                        *puVar9 = plVar8;
                        if (plVar8 == (long *)0x0) break;
                        lVar10 = plVar7[-1];
                        *plVar8 = (long)puVar9;
                        *plVar7 = (long)(plVar7 + 1);
                        plVar7[1] = (long)plVar7;
                        (**(code **)(lVar10 + 0x10))();
                        plVar7 = (long *)puVar12[-0xd];
                        if (plVar7 == (long *)0x0) goto LAB_0290eb64;
                      }
                      *plVar7 = (long)(plVar7 + 1);
                      plVar7[1] = (long)plVar7;
                      (**(code **)(plVar7[-1] + 0x10))(plVar7 + -1);
                      plVar7 = (long *)puVar12[-0xd];
                    }
                  }
                  else {
                    (**(code **)*puVar5)();
                  }
LAB_0290eb64:
                  puVar12 = puVar5;
                } while (puVar15 != puVar5);
                operator_delete(puVar15,0xe0);
                puVar5 = *(undefined8 **)(param_1 + 0x2450);
                puVar12 = puVar5 + 0xe;
              }
              FUN_01cfe2b0(0,puVar5);
              FUN_01cfe2b0(0,puVar12);
            }
            goto LAB_0290e16c;
          }
        }
      }
    }
  }
  else {
    FUN_0286e7b0(param_1,param_2,param_4);
    FUN_02879c80(param_1 + 0xb38,param_2 + 0xb38);
    puVar11 = (undefined4 *)(param_2 + 0xd48);
    lVar10 = param_1 + 0xe58;
    do {
      puVar1 = puVar11 + 0x16;
      FUN_014b16a0(lVar10 + -0x160,*puVar11);
      FUN_014b16a0(lVar10,puVar11[0x58]);
      puVar11 = puVar1;
      lVar10 = lVar10 + 0x58;
    } while (puVar1 != (undefined4 *)(param_2 + 0xea8));
    if (*(char *)(param_2 + 0xcf3) == '\0') {
      FUN_014af150(param_1 + 0xcd8,0);
    }
    else {
      FUN_014af150(param_1 + 0xcd8,1);
    }
    FUN_014b16a0(param_1 + 0x1538,*(undefined4 *)(param_2 + 0x1588));
    FUN_01db15f0(param_1 + 0x1590,*(undefined4 *)(param_2 + 0x15e0));
    FUN_0160b380(param_1 + 0x15e8,*(undefined4 *)(param_2 + 0x1638));
    FUN_014b3740(param_1 + 0x1640,*(undefined4 *)(param_2 + 0x1690));
    FUN_014904f0(param_1 + 0x1698,*(undefined1 *)(param_2 + 0x16e8));
    local_bd8 = *(undefined4 *)(param_2 + 0x1740);
    if (*(long *)(param_1 + 0x1758) != 0) {
      uVar3 = (**(code **)(param_1 + 0x1760))(param_1 + 0x1748,&local_bd8);
      FUN_014b16a0(param_1 + 0x16f0,uVar3);
      local_bd4 = *(undefined4 *)(param_2 + 0x17b8);
      if (*(long *)(param_1 + 0x17d0) != 0) {
        uVar3 = (**(code **)(param_1 + 0x17d8))(param_1 + 0x17c0,&local_bd4);
        FUN_014b16a0(param_1 + 0x1768,uVar3);
        uVar3 = *(undefined4 *)(param_2 + 0x1f68);
        *(undefined8 *)(param_1 + 0x2460) = *(undefined8 *)(param_2 + 0x2460);
        FUN_014b16a0(param_1 + 0x1f18,uVar3);
        FUN_014904f0(param_1 + 0x1fc8,*(undefined1 *)(param_2 + 0x2018));
        FUN_014904f0(param_1 + 0x2020,*(undefined1 *)(param_2 + 0x2070));
        local_bd0 = CONCAT44(local_bd0._4_4_,*(undefined4 *)(param_2 + 0x20c8));
        if (*(long *)(param_1 + 0x20e0) != 0) {
          (**(code **)(param_1 + 0x20e8))(param_1 + 0x20d0,&local_bd0);
          FUN_014bb930(param_1 + 0x2078);
          local_bb8[0] = (long *)CONCAT44(local_bb8[0]._4_4_,*(undefined4 *)(param_2 + 0x2140));
          if (*(long *)(param_1 + 0x2158) != 0) {
            uVar3 = (**(code **)(param_1 + 0x2160))(param_1 + 0x2148,local_bb8);
            FUN_014b3740(param_1 + 0x20f0,uVar3);
            lVar10 = 0;
            FUN_015916f0(param_1 + 0x21e0,*(undefined4 *)(param_2 + 0x2230));
            FUN_014b16a0(param_1 + 0x2238,*(undefined4 *)(param_2 + 0x2288));
            FUN_014b16a0(param_1 + 0x2290,*(undefined4 *)(param_2 + 0x22e0));
            do {
              lVar16 = *(long *)(param_1 + 0x22e8);
              FUN_0291aad0(auStack_b98,*(long *)(param_2 + 0x22e8) + lVar10);
                    /* try { // try from 0290df50 to 0290df53 has its CatchHandler @ 0290eda8 */
              FUN_02918be0(lVar16 + lVar10,auStack_b98);
              lVar10 = lVar10 + 0xb30;
              FUN_02914410(auStack_b98);
            } while (lVar10 != 0x5980);
            uVar2 = *(undefined1 *)(param_2 + 0x234c);
            uVar3 = *(undefined4 *)(param_2 + 0x2350);
            *(undefined4 *)(param_1 + 0x2348) = *(undefined4 *)(param_2 + 0x2348);
            *(undefined1 *)(param_1 + 0x234c) = uVar2;
            uVar17 = *(undefined4 *)(param_2 + 0x2358);
            *(undefined1 *)(param_1 + 0x234d) = *(undefined1 *)(param_2 + 0x234d);
            *(undefined1 *)(param_1 + 0x234e) = *(undefined1 *)(param_2 + 0x234e);
            *(undefined4 *)(param_1 + 0x2350) = uVar3;
            *(undefined1 *)(param_1 + 0x2354) = *(undefined1 *)(param_2 + 0x2354);
            *(undefined1 *)(param_1 + 0x2355) = *(undefined1 *)(param_2 + 0x2355);
            uVar2 = *(undefined1 *)(param_2 + 0x235c);
            *(undefined4 *)(param_1 + 0x2358) = uVar17;
            *(undefined1 *)(param_1 + 0x235c) = uVar2;
            FUN_014904f0(param_1 + 0x1f70,*(undefined1 *)(param_2 + 0x1fc0));
            lVar10 = *(long *)(param_2 + 0x2458);
            if (lVar10 == 0) {
              plVar7 = *(long **)(param_1 + 0x2458);
              *(undefined8 *)(param_1 + 0x2458) = 0;
joined_r0x0290e7b4:
              if (plVar7 != (long *)0x0) {
                if (*(code **)(*plVar7 + 8) == (code *)&LAB_015f2770) {
                  *plVar7 = (long)&PTR_FUN_06e8ca20;
                  FUN_0246f110(plVar7 + 0x43);
                  FUN_0246f110(plVar7 + 0x38);
                  FUN_0246f110(plVar7 + 0x2d);
                  FUN_0246f110(plVar7 + 0x22);
                  FUN_0246f110(plVar7 + 0x17);
                  FUN_0246f110(plVar7 + 0xc);
                  FUN_0246f110(plVar7 + 1);
                  operator_delete(plVar7,0x270);
                }
                else {
                  (**(code **)(*plVar7 + 8))(plVar7);
                }
              }
            }
            else {
              lVar16 = *(long *)(param_1 + 0x2458);
              if (lVar16 == 0) {
                pvVar4 = operator_new(0x270);
                    /* try { // try from 0290ea10 to 0290ea13 has its CatchHandler @ 0290ed34 */
                FUN_021905a0(pvVar4,lVar10);
                plVar7 = *(long **)(param_1 + 0x2458);
                *(void **)(param_1 + 0x2458) = pvVar4;
                goto joined_r0x0290e7b4;
              }
              FUN_02191300(lVar16 + 8,*(undefined4 *)(lVar10 + 0x58));
              FUN_014b3740(lVar16 + 0x60,*(undefined4 *)(lVar10 + 0xb0));
              FUN_014b3740(lVar16 + 0xb8,*(undefined4 *)(lVar10 + 0x108));
              FUN_014b3740(lVar16 + 0x110,*(undefined4 *)(lVar10 + 0x160));
              FUN_014b3740(lVar16 + 0x168,*(undefined4 *)(lVar10 + 0x1b8));
              FUN_014b3740(lVar16 + 0x1c0,*(undefined4 *)(lVar10 + 0x210));
              FUN_014b3740(lVar16 + 0x218,*(undefined4 *)(lVar10 + 0x268));
            }
            FUN_02922260(*(undefined8 *)(param_1 + 0x2340),*(undefined8 *)(param_2 + 0x2340));
            puVar11 = (undefined4 *)(param_2 + 0x1008);
            lVar10 = param_1 + 0x1278;
            do {
              puVar1 = puVar11 + 0x16;
              FUN_014b16a0(lVar10 + -0x2c0,*puVar11);
              FUN_014bb930(puVar11[0xb0],lVar10);
              puVar11 = puVar1;
              lVar10 = lVar10 + 0x58;
            } while ((undefined4 *)(param_2 + 0x12c8) != puVar1);
            FUN_02576c30(param_1 + 0x2368,param_2 + 0x2368);
            lVar10 = *(long *)(param_2 + 0x2440);
            if (lVar10 == 0) {
              pvVar4 = *(void **)(param_1 + 0x2440);
              *(undefined8 *)(param_1 + 0x2440) = 0;
joined_r0x0290e79c:
              if (pvVar4 != (void *)0x0) {
                operator_delete(pvVar4,0x88);
              }
            }
            else {
              if (*(long *)(param_1 + 0x2440) == 0) {
                pvVar6 = operator_new(0x88);
                    /* try { // try from 0290e9ec to 0290e9ef has its CatchHandler @ 0290eee4 */
                FUN_028f9f80(pvVar6,lVar10);
                pvVar4 = *(void **)(param_1 + 0x2440);
                *(void **)(param_1 + 0x2440) = pvVar6;
                goto joined_r0x0290e79c;
              }
              FUN_028fa000(*(long *)(param_1 + 0x2440),lVar10);
            }
            lVar10 = param_1 + 0x1838;
            FUN_014904f0(param_1 + 0x17e0,*(undefined1 *)(param_2 + 0x1830));
            puVar13 = (undefined1 *)(param_2 + 0x1888);
            do {
              FUN_02913b80(lVar10,*puVar13);
              FUN_014bb930(*(undefined4 *)(puVar13 + 0x160),lVar10 + 0x160);
              FUN_014bb930(*(undefined4 *)(puVar13 + 0x2c0),lVar10 + 0x2c0);
              FUN_014bb930(*(undefined4 *)(puVar13 + 0x420),lVar10 + 0x420);
              lVar16 = lVar10 + 0x580;
              lVar10 = lVar10 + 0x58;
              FUN_02913d40(lVar16,*(undefined4 *)(puVar13 + 0x580));
              puVar13 = puVar13 + 0x58;
            } while (lVar10 != param_1 + 0x1998);
            lVar10 = *(long *)(param_2 + 0x2450);
            puVar12 = *(undefined8 **)(param_1 + 0x2450);
            if (lVar10 == 0) {
              *(undefined8 *)(param_1 + 0x2450) = 0;
              if (puVar12 != (undefined8 *)0x0) {
                puVar5 = puVar12 + 0x1c;
                do {
                  puVar15 = puVar5 + -0xe;
                  if (*(code **)*puVar15 == FUN_01cfe380) {
                    *puVar15 = &PTR_FUN_06e9b878;
                    FUN_0246f110(puVar5 + -0xc);
                    plVar7 = (long *)puVar5[-0xd];
                    while (plVar7 != (long *)0x0) {
                      while( true ) {
                        plVar8 = (long *)plVar7[1];
                        puVar9 = (undefined8 *)*plVar7;
                        *puVar9 = plVar8;
                        if (plVar8 == (long *)0x0) break;
                        lVar10 = plVar7[-1];
                        *plVar8 = (long)puVar9;
                        *plVar7 = (long)(plVar7 + 1);
                        plVar7[1] = (long)plVar7;
                        (**(code **)(lVar10 + 0x10))();
                        plVar7 = (long *)puVar5[-0xd];
                        if (plVar7 == (long *)0x0) goto LAB_0290e75c;
                      }
                      *plVar7 = (long)(plVar7 + 1);
                      plVar7[1] = (long)plVar7;
                      (**(code **)(plVar7[-1] + 0x10))(plVar7 + -1);
                      plVar7 = (long *)puVar5[-0xd];
                    }
                  }
                  else {
                    (**(code **)*puVar15)();
                  }
LAB_0290e75c:
                  puVar5 = puVar15;
                } while (puVar12 != puVar15);
                operator_delete(puVar12,0xe0);
              }
            }
            else if (puVar12 == (undefined8 *)0x0) {
              pvVar4 = operator_new(0xe0);
                    /* try { // try from 0290ec1c to 0290ec1f has its CatchHandler @ 0290ee90 */
              FUN_01cfe020(pvVar4,lVar10);
                    /* try { // try from 0290ec28 to 0290ec2b has its CatchHandler @ 0290ee40 */
              FUN_01cfe020((long)pvVar4 + 0x70,lVar10 + 0x70);
              puVar12 = *(undefined8 **)(param_1 + 0x2450);
              *(void **)(param_1 + 0x2450) = pvVar4;
              if (puVar12 != (undefined8 *)0x0) {
                puVar5 = puVar12 + 0x1c;
                do {
                  puVar15 = puVar5 + -0xe;
                  if (*(code **)*puVar15 == FUN_01cfe380) {
                    *puVar15 = &PTR_FUN_06e9b878;
                    FUN_0246f110(puVar5 + -0xc);
                    plVar7 = (long *)puVar5[-0xd];
                    while (plVar7 != (long *)0x0) {
                      while( true ) {
                        plVar8 = (long *)plVar7[1];
                        puVar9 = (undefined8 *)*plVar7;
                        *puVar9 = plVar8;
                        if (plVar8 == (long *)0x0) break;
                        lVar10 = plVar7[-1];
                        *plVar8 = (long)puVar9;
                        *plVar7 = (long)(plVar7 + 1);
                        plVar7[1] = (long)plVar7;
                        (**(code **)(lVar10 + 0x10))();
                        plVar7 = (long *)puVar5[-0xd];
                        if (plVar7 == (long *)0x0) goto LAB_0290eca4;
                      }
                      *plVar7 = (long)(plVar7 + 1);
                      plVar7[1] = (long)plVar7;
                      (**(code **)(plVar7[-1] + 0x10))(plVar7 + -1);
                      plVar7 = (long *)puVar5[-0xd];
                    }
                  }
                  else {
                    (**(code **)*puVar15)();
                  }
LAB_0290eca4:
                  puVar5 = puVar15;
                } while (puVar12 != puVar15);
                operator_delete(puVar12,0xe0);
              }
            }
            else {
              FUN_01cfe270(puVar12,lVar10);
              FUN_01cfe270(puVar12 + 0xe,lVar10 + 0x70);
            }
            pvVar6 = *(void **)(param_2 + 0x2448);
            pvVar4 = *(void **)(param_1 + 0x2448);
            if (pvVar6 == (void *)0x0) {
              *(undefined8 *)(param_1 + 0x2448) = 0;
joined_r0x0290e6d4:
              if (pvVar4 != (void *)0x0) {
                operator_delete(pvVar4,0x680);
              }
            }
            else {
              if (pvVar4 == (void *)0x0) {
                __dest = operator_new(0x680);
                memcpy(__dest,pvVar6,0x680);
                pvVar4 = *(void **)(param_1 + 0x2448);
                *(void **)(param_1 + 0x2448) = __dest;
                goto joined_r0x0290e6d4;
              }
              memcpy(pvVar4,pvVar6,0x680);
            }
            FUN_02904c50(param_1 + 0x2448,*(undefined8 *)(param_1 + 0x2310));
LAB_0290e16c:
            if ((*(byte *)(param_1 + 0x3d8) & 1) != 0) {
              memset(*(void **)(param_1 + 0x3c0),0,
                     *(long *)(param_1 + 0x3c8) - (long)*(void **)(param_1 + 0x3c0));
              *(undefined1 *)(param_1 + 0x3d8) = 0;
            }
            lVar10 = local_68 - ___stack_chk_guard;
            if (lVar10 == 0) {
              return;
            }
            goto LAB_0290ed64;
          }
        }
      }
    }
  }
  lVar10 = local_68 - ___stack_chk_guard;
  if (lVar10 == 0) {
                    /* WARNING: Subroutine does not return */
    std::__throw_bad_function_call();
  }
LAB_0290ed64:
                    /* WARNING: Subroutine does not return */
  __stack_chk_fail(&__stack_chk_guard,0,lVar10);
}

