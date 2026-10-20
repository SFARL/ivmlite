Validated rows: ablation=100, confirm=240, demos=92, write_amp=48
Before/after extension SHAs: before=b647bcba9fa5f8eb583b162f87f7dfd79a10f7fa, after=84e4c07c2ebfc9db8b6bafb57378627414df5524
SQLite: 3.53.2; all scoped harnesses clean.
Ratios are after/before (>1 is slower). Values are median [min, max]. Only maintain_ms and apply_plus_maintain_ms enter the integer 1.05 check. 0/0 is n/a; a positive result over zero is inf.

Integer ablation

| cell (views/base/batch/groups), engine | metric | before | after | ratio | flag |
|---|---|---:|---:|---:|---|
| 10/100000/1000/1000/ivmlite | bootstrap_ms | 1132.701 [942.094, 1170.745] | 960.327 [880.693, 1067.087] | 0.848 |  |
| 10/100000/1000/1000/ivmlite | first_refresh_ms | 66.685 [58.139, 212.584] | 58.509 [54.724, 61.029] | 0.877 |  |
| 10/100000/1000/1000/ivmlite | apply_ms | 8.622 [7.803, 8.908] | 8.187 [7.810, 8.474] | 0.950 |  |
| 10/100000/1000/1000/ivmlite | maintain_ms | 58.051 [55.412, 61.055] | 56.669 [53.555, 57.194] | 0.976 |  |
| 10/100000/1000/1000/ivmlite | apply_plus_maintain_ms | 66.513 [63.568, 68.691] | 64.628 [61.155, 65.965] | 0.972 |  |
| 10/100000/1000/1000/naive_recompute | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 10/100000/1000/1000/naive_recompute | first_refresh_ms | 267.598 [248.208, 406.137] | 255.357 [241.255, 275.708] | 0.954 |  |
| 10/100000/1000/1000/naive_recompute | apply_ms | 0.544 [0.476, 0.614] | 0.485 [0.449, 0.587] | 0.892 |  |
| 10/100000/1000/1000/naive_recompute | maintain_ms | 280.079 [255.938, 325.238] | 264.704 [242.631, 348.716] | 0.945 |  |
| 10/100000/1000/1000/naive_recompute | apply_plus_maintain_ms | 280.514 [256.487, 325.861] | 265.184 [243.197, 350.151] | 0.945 |  |
| 10/100000/1000/100000/ivmlite | bootstrap_ms | 4330.966 [4260.791, 4445.009] | 4599.014 [4562.872, 4648.475] | 1.062 |  |
| 10/100000/1000/100000/ivmlite | first_refresh_ms | 96.688 [87.771, 119.283] | 87.519 [86.687, 96.756] | 0.905 |  |
| 10/100000/1000/100000/ivmlite | apply_ms | 7.918 [7.819, 8.175] | 7.963 [7.778, 8.221] | 1.006 |  |
| 10/100000/1000/100000/ivmlite | maintain_ms | 95.617 [90.535, 100.935] | 93.776 [89.823, 96.296] | 0.981 |  |
| 10/100000/1000/100000/ivmlite | apply_plus_maintain_ms | 103.823 [98.178, 109.017] | 101.591 [97.599, 104.392] | 0.979 |  |
| 10/100000/1000/100000/naive_recompute | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 10/100000/1000/100000/naive_recompute | first_refresh_ms | 405.360 [315.221, 658.683] | 320.895 [316.242, 322.777] | 0.792 |  |
| 10/100000/1000/100000/naive_recompute | apply_ms | 0.567 [0.502, 0.954] | 0.557 [0.510, 0.619] | 0.982 |  |
| 10/100000/1000/100000/naive_recompute | maintain_ms | 398.368 [391.140, 437.457] | 402.127 [390.873, 412.054] | 1.009 |  |
| 10/100000/1000/100000/naive_recompute | apply_plus_maintain_ms | 398.985 [391.668, 443.006] | 402.695 [391.383, 412.758] | 1.009 |  |
| 200/100000/1/1000/ivmlite | bootstrap_ms | 15350.819 [15024.347, 16424.307] | 15436.897 [15398.727, 15814.632] | 1.006 |  |
| 200/100000/1/1000/ivmlite | first_refresh_ms | 21.559 [20.070, 23.861] | 20.538 [20.350, 21.182] | 0.953 |  |
| 200/100000/1/1000/ivmlite | apply_ms | 0.081 [0.069, 0.145] | 0.071 [0.066, 0.157] | 0.877 |  |
| 200/100000/1/1000/ivmlite | maintain_ms | 6.155 [5.702, 7.220] | 5.822 [5.793, 7.099] | 0.946 |  |
| 200/100000/1/1000/ivmlite | apply_plus_maintain_ms | 6.160 [5.708, 7.226] | 5.828 [5.799, 7.111] | 0.946 |  |
| 200/100000/1/1000/naive_recompute | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 200/100000/1/1000/naive_recompute | first_refresh_ms | 3654.180 [3556.342, 4012.532] | 3539.669 [3525.958, 3919.440] | 0.969 |  |
| 200/100000/1/1000/naive_recompute | apply_ms | 0.009 [0.009, 0.012] | 0.009 [0.009, 0.013] | 1.000 |  |
| 200/100000/1/1000/naive_recompute | maintain_ms | 3559.094 [3551.284, 3779.496] | 3549.907 [3545.548, 3773.232] | 0.997 |  |
| 200/100000/1/1000/naive_recompute | apply_plus_maintain_ms | 3559.103 [3551.292, 3779.506] | 3549.915 [3545.557, 3773.245] | 0.997 |  |
| 200/100000/100/100000/ivmlite | bootstrap_ms | 64978.075 [64224.424, 65422.074] | 66681.364 [66241.986, 69229.497] | 1.026 |  |
| 200/100000/100/100000/ivmlite | first_refresh_ms | 285.182 [189.024, 305.707] | 220.528 [187.879, 319.227] | 0.773 |  |
| 200/100000/100/100000/ivmlite | apply_ms | 4.127 [4.049, 4.454] | 4.176 [4.137, 4.291] | 1.012 |  |
| 200/100000/100/100000/ivmlite | maintain_ms | 232.089 [182.356, 258.269] | 196.353 [149.724, 201.382] | 0.846 |  |
| 200/100000/100/100000/ivmlite | apply_plus_maintain_ms | 236.504 [186.207, 262.910] | 200.716 [153.783, 205.766] | 0.849 |  |
| 200/100000/100/100000/naive_recompute | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 200/100000/100/100000/naive_recompute | first_refresh_ms | 6256.560 [6244.044, 6552.095] | 6152.253 [6123.971, 6176.032] | 0.983 |  |
| 200/100000/100/100000/naive_recompute | apply_ms | 0.101 [0.073, 0.155] | 0.080 [0.074, 0.114] | 0.792 |  |
| 200/100000/100/100000/naive_recompute | maintain_ms | 6216.367 [6183.520, 6513.238] | 6199.829 [6151.852, 6803.578] | 0.997 |  |
| 200/100000/100/100000/naive_recompute | apply_plus_maintain_ms | 6216.449 [6183.591, 6513.441] | 6199.915 [6151.930, 6803.684] | 0.997 |  |
| 200/100000/1000/1000/ivmlite | bootstrap_ms | 16843.284 [15324.853, 17948.990] | 16745.188 [16289.080, 17513.081] | 0.994 |  |
| 200/100000/1000/1000/ivmlite | first_refresh_ms | 1018.957 [942.182, 1227.757] | 937.335 [929.497, 973.707] | 0.920 |  |
| 200/100000/1000/1000/ivmlite | apply_ms | 40.299 [38.457, 41.706] | 41.466 [40.214, 42.036] | 1.029 |  |
| 200/100000/1000/1000/ivmlite | maintain_ms | 962.505 [912.251, 980.843] | 953.161 [940.946, 976.416] | 0.990 |  |
| 200/100000/1000/1000/ivmlite | apply_plus_maintain_ms | 1001.162 [950.708, 1030.906] | 995.168 [993.424, 1018.442] | 0.994 |  |
| 200/100000/1000/1000/naive_recompute | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 200/100000/1000/1000/naive_recompute | first_refresh_ms | 3778.852 [3599.294, 4186.340] | 3675.090 [3559.025, 3772.828] | 0.973 |  |
| 200/100000/1000/1000/naive_recompute | apply_ms | 0.787 [0.578, 0.975] | 0.652 [0.557, 0.664] | 0.828 |  |
| 200/100000/1000/1000/naive_recompute | maintain_ms | 3798.184 [3624.510, 4007.241] | 3698.674 [3644.062, 3747.887] | 0.974 |  |
| 200/100000/1000/1000/naive_recompute | apply_plus_maintain_ms | 3798.933 [3625.075, 4008.215] | 3699.372 [3644.613, 3748.420] | 0.974 |  |

Integer confirm

| cell (views/base/batch/groups), engine | metric | before | after | ratio | flag |
|---|---|---:|---:|---:|---|
| 1/10000/100/1000/hand_written_trigger | bootstrap_ms | 2.680 [2.377, 3.826] | 2.660 [2.535, 4.483] | 0.993 |  |
| 1/10000/100/1000/hand_written_trigger | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 1/10000/100/1000/hand_written_trigger | apply_ms | 0.180 [0.171, 0.229] | 0.184 [0.171, 0.263] | 1.022 |  |
| 1/10000/100/1000/hand_written_trigger | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 1/10000/100/1000/hand_written_trigger | apply_plus_maintain_ms | 0.180 [0.171, 0.229] | 0.184 [0.171, 0.263] | 1.022 |  |
| 1/10000/100/1000/ivmlite | bootstrap_ms | 12.183 [11.806, 22.050] | 12.736 [12.130, 22.269] | 1.045 |  |
| 1/10000/100/1000/ivmlite | first_refresh_ms | 1.115 [1.079, 1.484] | 1.129 [1.098, 1.274] | 1.013 |  |
| 1/10000/100/1000/ivmlite | apply_ms | 0.560 [0.539, 0.703] | 0.547 [0.530, 0.693] | 0.977 |  |
| 1/10000/100/1000/ivmlite | maintain_ms | 0.848 [0.798, 1.078] | 0.856 [0.785, 1.064] | 1.009 |  |
| 1/10000/100/1000/ivmlite | apply_plus_maintain_ms | 1.395 [1.336, 1.818] | 1.403 [1.329, 1.766] | 1.006 |  |
| 1/10000/100/1000/naive_recompute | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 1/10000/100/1000/naive_recompute | first_refresh_ms | 2.262 [2.150, 2.647] | 2.350 [2.216, 3.148] | 1.039 |  |
| 1/10000/100/1000/naive_recompute | apply_ms | 0.045 [0.036, 0.050] | 0.039 [0.034, 0.056] | 0.867 |  |
| 1/10000/100/1000/naive_recompute | maintain_ms | 2.194 [2.171, 2.546] | 2.328 [2.237, 2.773] | 1.061 | >1.05 |
| 1/10000/100/1000/naive_recompute | apply_plus_maintain_ms | 2.232 [2.229, 2.601] | 2.370 [2.272, 2.815] | 1.062 | >1.05 |
| 1/10000/100/1000/no_maintenance | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 1/10000/100/1000/no_maintenance | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 1/10000/100/1000/no_maintenance | apply_ms | 0.034 [0.034, 0.057] | 0.035 [0.035, 0.052] | 1.029 |  |
| 1/10000/100/1000/no_maintenance | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 1/10000/100/1000/no_maintenance | apply_plus_maintain_ms | 0.034 [0.034, 0.057] | 0.035 [0.035, 0.052] | 1.029 |  |
| 1/100000/1000/1000/hand_written_trigger | bootstrap_ms | 29.088 [28.558, 29.992] | 30.159 [28.856, 33.982] | 1.037 |  |
| 1/100000/1000/1000/hand_written_trigger | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 1/100000/1000/1000/hand_written_trigger | apply_ms | 1.746 [1.731, 1.995] | 1.777 [1.713, 2.008] | 1.018 |  |
| 1/100000/1000/1000/hand_written_trigger | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 1/100000/1000/1000/hand_written_trigger | apply_plus_maintain_ms | 1.746 [1.731, 1.995] | 1.777 [1.713, 2.008] | 1.018 |  |
| 1/100000/1000/1000/ivmlite | bootstrap_ms | 95.362 [91.917, 97.998] | 96.573 [89.057, 103.952] | 1.013 |  |
| 1/100000/1000/1000/ivmlite | first_refresh_ms | 6.115 [6.040, 6.587] | 6.147 [6.117, 6.581] | 1.005 |  |
| 1/100000/1000/1000/ivmlite | apply_ms | 5.766 [5.481, 5.996] | 5.540 [5.425, 5.841] | 0.961 |  |
| 1/100000/1000/1000/ivmlite | maintain_ms | 5.857 [5.593, 5.911] | 5.605 [5.567, 5.861] | 0.957 |  |
| 1/100000/1000/1000/ivmlite | apply_plus_maintain_ms | 11.564 [11.217, 11.752] | 11.215 [11.073, 11.691] | 0.970 |  |
| 1/100000/1000/1000/naive_recompute | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 1/100000/1000/1000/naive_recompute | first_refresh_ms | 28.393 [27.716, 28.865] | 27.850 [26.989, 29.793] | 0.981 |  |
| 1/100000/1000/1000/naive_recompute | apply_ms | 0.444 [0.416, 0.523] | 0.368 [0.364, 0.424] | 0.829 |  |
| 1/100000/1000/1000/naive_recompute | maintain_ms | 27.877 [27.748, 28.825] | 28.111 [27.558, 28.812] | 1.008 |  |
| 1/100000/1000/1000/naive_recompute | apply_plus_maintain_ms | 28.348 [28.161, 29.285] | 28.462 [27.926, 29.236] | 1.004 |  |
| 1/100000/1000/1000/no_maintenance | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 1/100000/1000/1000/no_maintenance | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 1/100000/1000/1000/no_maintenance | apply_ms | 0.377 [0.353, 0.387] | 0.385 [0.341, 0.403] | 1.021 |  |
| 1/100000/1000/1000/no_maintenance | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 1/100000/1000/1000/no_maintenance | apply_plus_maintain_ms | 0.377 [0.353, 0.387] | 0.385 [0.341, 0.403] | 1.021 |  |
| 10/10000/100/1000/hand_written_trigger | bootstrap_ms | 21.158 [20.886, 21.240] | 21.377 [20.732, 21.699] | 1.010 |  |
| 10/10000/100/1000/hand_written_trigger | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 10/10000/100/1000/hand_written_trigger | apply_ms | 1.104 [1.085, 1.157] | 1.100 [1.096, 1.130] | 0.996 |  |
| 10/10000/100/1000/hand_written_trigger | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 10/10000/100/1000/hand_written_trigger | apply_plus_maintain_ms | 1.104 [1.085, 1.157] | 1.100 [1.096, 1.130] | 0.996 |  |
| 10/10000/100/1000/ivmlite | bootstrap_ms | 106.473 [103.909, 109.255] | 113.112 [106.399, 116.821] | 1.062 |  |
| 10/10000/100/1000/ivmlite | first_refresh_ms | 9.068 [9.028, 9.912] | 9.470 [9.067, 9.590] | 1.044 |  |
| 10/10000/100/1000/ivmlite | apply_ms | 0.742 [0.712, 0.777] | 0.727 [0.717, 0.758] | 0.980 |  |
| 10/10000/100/1000/ivmlite | maintain_ms | 7.047 [6.950, 7.221] | 6.893 [6.856, 7.177] | 0.978 |  |
| 10/10000/100/1000/ivmlite | apply_plus_maintain_ms | 7.795 [7.701, 8.025] | 7.640 [7.595, 7.923] | 0.980 |  |
| 10/10000/100/1000/naive_recompute | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 10/10000/100/1000/naive_recompute | first_refresh_ms | 19.462 [19.344, 19.679] | 19.781 [19.597, 20.519] | 1.016 |  |
| 10/10000/100/1000/naive_recompute | apply_ms | 0.044 [0.039, 0.045] | 0.041 [0.036, 0.052] | 0.932 |  |
| 10/10000/100/1000/naive_recompute | maintain_ms | 20.568 [20.471, 20.982] | 20.795 [20.552, 21.533] | 1.011 |  |
| 10/10000/100/1000/naive_recompute | apply_plus_maintain_ms | 20.616 [20.510, 21.023] | 20.833 [20.598, 21.579] | 1.011 |  |
| 10/10000/100/1000/no_maintenance | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 10/10000/100/1000/no_maintenance | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 10/10000/100/1000/no_maintenance | apply_ms | 0.034 [0.034, 0.036] | 0.034 [0.034, 0.040] | 1.000 |  |
| 10/10000/100/1000/no_maintenance | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 10/10000/100/1000/no_maintenance | apply_plus_maintain_ms | 0.034 [0.034, 0.036] | 0.034 [0.034, 0.040] | 1.000 |  |
| 10/10000/1000/10/hand_written_trigger | bootstrap_ms | 17.155 [16.874, 17.781] | 17.311 [17.248, 17.939] | 1.009 |  |
| 10/10000/1000/10/hand_written_trigger | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 10/10000/1000/10/hand_written_trigger | apply_ms | 9.350 [9.019, 9.467] | 9.100 [8.985, 9.454] | 0.973 |  |
| 10/10000/1000/10/hand_written_trigger | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 10/10000/1000/10/hand_written_trigger | apply_plus_maintain_ms | 9.350 [9.019, 9.467] | 9.100 [8.985, 9.454] | 0.973 |  |
| 10/10000/1000/10/ivmlite | bootstrap_ms | 59.712 [58.828, 62.399] | 62.254 [60.473, 74.945] | 1.043 |  |
| 10/10000/1000/10/ivmlite | first_refresh_ms | 7.721 [7.589, 8.057] | 8.079 [7.713, 8.161] | 1.046 |  |
| 10/10000/1000/10/ivmlite | apply_ms | 7.369 [7.257, 7.587] | 7.344 [7.166, 7.417] | 0.997 |  |
| 10/10000/1000/10/ivmlite | maintain_ms | 6.668 [6.429, 6.816] | 6.920 [6.497, 7.001] | 1.038 |  |
| 10/10000/1000/10/ivmlite | apply_plus_maintain_ms | 14.020 [13.709, 14.080] | 14.223 [13.695, 14.303] | 1.014 |  |
| 10/10000/1000/10/naive_recompute | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 10/10000/1000/10/naive_recompute | first_refresh_ms | 17.040 [16.734, 17.234] | 17.326 [17.206, 17.630] | 1.017 |  |
| 10/10000/1000/10/naive_recompute | apply_ms | 0.272 [0.256, 0.285] | 0.261 [0.253, 0.277] | 0.960 |  |
| 10/10000/1000/10/naive_recompute | maintain_ms | 19.137 [18.604, 19.689] | 19.191 [19.015, 19.400] | 1.003 |  |
| 10/10000/1000/10/naive_recompute | apply_plus_maintain_ms | 19.405 [18.861, 19.968] | 19.448 [19.268, 19.684] | 1.002 |  |
| 10/10000/1000/10/no_maintenance | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 10/10000/1000/10/no_maintenance | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 10/10000/1000/10/no_maintenance | apply_ms | 0.255 [0.248, 0.262] | 0.256 [0.251, 0.263] | 1.004 |  |
| 10/10000/1000/10/no_maintenance | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 10/10000/1000/10/no_maintenance | apply_plus_maintain_ms | 0.255 [0.248, 0.262] | 0.256 [0.251, 0.263] | 1.004 |  |
| 50/10000/100/1000/hand_written_trigger | bootstrap_ms | 88.523 [85.931, 88.878] | 89.565 [88.181, 108.816] | 1.012 |  |
| 50/10000/100/1000/hand_written_trigger | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 50/10000/100/1000/hand_written_trigger | apply_ms | 4.524 [4.486, 4.641] | 4.705 [4.481, 4.867] | 1.040 |  |
| 50/10000/100/1000/hand_written_trigger | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 50/10000/100/1000/hand_written_trigger | apply_plus_maintain_ms | 4.524 [4.486, 4.641] | 4.705 [4.481, 4.867] | 1.040 |  |
| 50/10000/100/1000/ivmlite | bootstrap_ms | 513.244 [511.980, 564.181] | 531.051 [520.817, 562.087] | 1.035 |  |
| 50/10000/100/1000/ivmlite | first_refresh_ms | 41.610 [41.248, 42.318] | 40.680 [40.299, 42.529] | 0.978 |  |
| 50/10000/100/1000/ivmlite | apply_ms | 1.462 [1.395, 1.493] | 1.500 [1.460, 1.526] | 1.026 |  |
| 50/10000/100/1000/ivmlite | maintain_ms | 30.323 [29.564, 30.662] | 29.951 [28.869, 31.137] | 0.988 |  |
| 50/10000/100/1000/ivmlite | apply_plus_maintain_ms | 31.990 [30.967, 32.057] | 31.401 [30.329, 32.709] | 0.982 |  |
| 50/10000/100/1000/naive_recompute | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 50/10000/100/1000/naive_recompute | first_refresh_ms | 86.356 [85.060, 86.688] | 89.023 [86.942, 100.592] | 1.031 |  |
| 50/10000/100/1000/naive_recompute | apply_ms | 0.060 [0.055, 0.063] | 0.058 [0.049, 0.063] | 0.967 |  |
| 50/10000/100/1000/naive_recompute | maintain_ms | 86.577 [86.332, 87.333] | 87.484 [87.295, 88.522] | 1.010 |  |
| 50/10000/100/1000/naive_recompute | apply_plus_maintain_ms | 86.655 [86.404, 87.396] | 87.535 [87.342, 88.586] | 1.010 |  |
| 50/10000/100/1000/no_maintenance | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 50/10000/100/1000/no_maintenance | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 50/10000/100/1000/no_maintenance | apply_ms | 0.034 [0.033, 0.037] | 0.034 [0.034, 0.035] | 1.000 |  |
| 50/10000/100/1000/no_maintenance | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 50/10000/100/1000/no_maintenance | apply_plus_maintain_ms | 0.034 [0.033, 0.037] | 0.034 [0.034, 0.035] | 1.000 |  |
| 200/10000/100/1000/hand_written_trigger | bootstrap_ms | 347.284 [338.577, 361.877] | 351.269 [350.335, 352.761] | 1.011 |  |
| 200/10000/100/1000/hand_written_trigger | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 200/10000/100/1000/hand_written_trigger | apply_ms | 19.346 [17.349, 22.168] | 19.862 [17.376, 21.268] | 1.027 |  |
| 200/10000/100/1000/hand_written_trigger | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 200/10000/100/1000/hand_written_trigger | apply_plus_maintain_ms | 19.346 [17.349, 22.168] | 19.862 [17.376, 21.268] | 1.027 |  |
| 200/10000/100/1000/ivmlite | bootstrap_ms | 2742.649 [2665.383, 2816.969] | 2658.446 [2641.152, 2685.679] | 0.969 |  |
| 200/10000/100/1000/ivmlite | first_refresh_ms | 169.078 [165.847, 174.189] | 156.887 [156.001, 162.904] | 0.928 |  |
| 200/10000/100/1000/ivmlite | apply_ms | 3.977 [3.830, 4.031] | 3.919 [3.895, 4.033] | 0.985 |  |
| 200/10000/100/1000/ivmlite | maintain_ms | 123.128 [116.438, 124.084] | 115.659 [115.177, 118.044] | 0.939 |  |
| 200/10000/100/1000/ivmlite | apply_plus_maintain_ms | 126.993 [120.415, 128.116] | 119.674 [119.088, 121.745] | 0.942 |  |
| 200/10000/100/1000/naive_recompute | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 200/10000/100/1000/naive_recompute | first_refresh_ms | 333.093 [325.578, 416.621] | 335.432 [333.513, 339.464] | 1.007 |  |
| 200/10000/100/1000/naive_recompute | apply_ms | 0.066 [0.059, 0.068] | 0.057 [0.056, 0.064] | 0.864 |  |
| 200/10000/100/1000/naive_recompute | maintain_ms | 338.524 [332.161, 345.821] | 339.159 [336.257, 340.614] | 1.002 |  |
| 200/10000/100/1000/naive_recompute | apply_plus_maintain_ms | 338.582 [332.227, 345.889] | 339.213 [336.310, 340.666] | 1.002 |  |
| 200/10000/100/1000/no_maintenance | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 200/10000/100/1000/no_maintenance | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 200/10000/100/1000/no_maintenance | apply_ms | 0.034 [0.033, 0.036] | 0.034 [0.034, 0.034] | 1.000 |  |
| 200/10000/100/1000/no_maintenance | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 200/10000/100/1000/no_maintenance | apply_plus_maintain_ms | 0.034 [0.033, 0.036] | 0.034 [0.034, 0.034] | 1.000 |  |

Unchanged integer demos

| case, mode | metric | before | after | ratio | flag |
|---|---|---:|---:|---:|---|
| nicti_facet_counts, handwritten_trigger | bootstrap_ms | 2860.263 [2788.079, 3002.864] | 2823.894 [2784.648, 2879.340] | 0.987 |  |
| nicti_facet_counts, handwritten_trigger | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| nicti_facet_counts, handwritten_trigger | apply_ms | 2.393 [2.332, 2.480] | 2.343 [2.320, 2.360] | 0.979 |  |
| nicti_facet_counts, handwritten_trigger | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| nicti_facet_counts, handwritten_trigger | read_ms | 0.039 [0.038, 0.040] | 0.038 [0.037, 0.039] | 0.974 |  |
| nicti_facet_counts, handwritten_trigger | apply_plus_maintain_ms | 2.393 [2.332, 2.480] | 2.343 [2.320, 2.360] | 0.979 |  |
| nicti_facet_counts, handwritten_trigger | end_to_end_ms | 2.430 [2.373, 2.520] | 2.381 [2.361, 2.396] | 0.980 |  |
| nicti_facet_counts, handwritten_trigger | database_kib | 180136.000 [180136.000, 180136.000] | 180136.000 [180136.000, 180136.000] | 1.000 |  |
| nicti_facet_counts, indexed_recompute | bootstrap_ms | 2015.105 [1938.182, 2065.745] | 1925.029 [1909.979, 1966.234] | 0.955 |  |
| nicti_facet_counts, indexed_recompute | first_refresh_ms | 2933.967 [2919.091, 2986.929] | 2876.254 [2864.525, 2900.919] | 0.980 |  |
| nicti_facet_counts, indexed_recompute | apply_ms | 0.364 [0.355, 0.512] | 0.348 [0.342, 0.355] | 0.956 |  |
| nicti_facet_counts, indexed_recompute | maintain_ms | 2967.137 [2883.819, 3016.783] | 2875.508 [2865.271, 2882.108] | 0.969 |  |
| nicti_facet_counts, indexed_recompute | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| nicti_facet_counts, indexed_recompute | apply_plus_maintain_ms | 2967.511 [2884.152, 3017.339] | 2875.842 [2865.603, 2882.451] | 0.969 |  |
| nicti_facet_counts, indexed_recompute | end_to_end_ms | 2967.511 [2884.152, 3017.339] | 2875.842 [2865.603, 2882.451] | 0.969 |  |
| nicti_facet_counts, indexed_recompute | database_kib | 312352.000 [312352.000, 312352.000] | 312352.000 [312352.000, 312352.000] | 1.000 |  |
| nicti_facet_counts, ivmlite | bootstrap_ms | 20044.383 [19368.912, 20441.537] | 19324.541 [18954.431, 20649.433] | 0.964 |  |
| nicti_facet_counts, ivmlite | first_refresh_ms | 9.163 [8.824, 9.793] | 11.277 [10.839, 12.418] | 1.231 |  |
| nicti_facet_counts, ivmlite | apply_ms | 2.308 [2.302, 2.327] | 2.295 [2.285, 2.312] | 0.994 |  |
| nicti_facet_counts, ivmlite | maintain_ms | 4.816 [4.807, 4.886] | 4.849 [4.833, 4.885] | 1.007 |  |
| nicti_facet_counts, ivmlite | read_ms | 0.214 [0.203, 0.220] | 0.209 [0.202, 0.212] | 0.977 |  |
| nicti_facet_counts, ivmlite | apply_plus_maintain_ms | 7.129 [7.105, 7.182] | 7.131 [7.112, 7.195] | 1.000 |  |
| nicti_facet_counts, ivmlite | end_to_end_ms | 7.352 [7.325, 7.406] | 7.348 [7.326, 7.414] | 0.999 |  |
| nicti_facet_counts, ivmlite | database_kib | 885736.000 [885736.000, 885736.000] | 885724.000 [885724.000, 885724.000] | 1.000 |  |
| nicti_facet_counts, no_maintenance | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| nicti_facet_counts, no_maintenance | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| nicti_facet_counts, no_maintenance | apply_ms | 0.133 [0.132, 0.134] | 0.131 [0.130, 0.136] | 0.985 |  |
| nicti_facet_counts, no_maintenance | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| nicti_facet_counts, no_maintenance | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| nicti_facet_counts, no_maintenance | apply_plus_maintain_ms | 0.133 [0.132, 0.134] | 0.131 [0.130, 0.136] | 0.985 |  |
| nicti_facet_counts, no_maintenance | end_to_end_ms | 0.133 [0.133, 0.134] | 0.131 [0.130, 0.136] | 0.985 |  |
| nicti_facet_counts, no_maintenance | database_kib | 180052.000 [180052.000, 180052.000] | 180052.000 [180052.000, 180052.000] | 1.000 |  |
| nicti_facet_counts, unindexed_recompute | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| nicti_facet_counts, unindexed_recompute | first_refresh_ms | 2858.416 [2844.922, 2906.848] | 2812.752 [2794.117, 2816.263] | 0.984 |  |
| nicti_facet_counts, unindexed_recompute | apply_ms | 0.174 [0.172, 0.179] | 0.172 [0.166, 0.212] | 0.989 |  |
| nicti_facet_counts, unindexed_recompute | maintain_ms | 2842.877 [2837.877, 2873.721] | 2806.185 [2797.201, 2927.985] | 0.987 |  |
| nicti_facet_counts, unindexed_recompute | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| nicti_facet_counts, unindexed_recompute | apply_plus_maintain_ms | 2843.054 [2838.050, 2873.905] | 2806.355 [2797.373, 2928.214] | 0.987 |  |
| nicti_facet_counts, unindexed_recompute | end_to_end_ms | 2843.054 [2838.050, 2873.905] | 2806.355 [2797.373, 2928.214] | 0.987 |  |
| nicti_facet_counts, unindexed_recompute | database_kib | 180052.000 [180052.000, 180052.000] | 180052.000 [180052.000, 180052.000] | 1.000 |  |
| noop_gravity_witness, indexed_recompute | bootstrap_ms | 152.408 [149.640, 155.161] | 150.494 [149.129, 151.808] | 0.987 |  |
| noop_gravity_witness, indexed_recompute | first_refresh_ms | 17.351 [17.031, 17.997] | 17.060 [17.039, 17.148] | 0.983 |  |
| noop_gravity_witness, indexed_recompute | apply_ms | 0.329 [0.320, 0.443] | 0.308 [0.306, 0.310] | 0.936 |  |
| noop_gravity_witness, indexed_recompute | maintain_ms | 17.364 [17.325, 17.854] | 17.046 [17.031, 17.054] | 0.982 |  |
| noop_gravity_witness, indexed_recompute | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| noop_gravity_witness, indexed_recompute | apply_plus_maintain_ms | 17.701 [17.672, 18.306] | 17.353 [17.334, 17.395] | 0.980 |  |
| noop_gravity_witness, indexed_recompute | end_to_end_ms | 17.701 [17.672, 18.306] | 17.353 [17.334, 17.395] | 0.980 |  |
| noop_gravity_witness, indexed_recompute | database_kib | 20852.000 [20852.000, 20852.000] | 20852.000 [20852.000, 20852.000] | 1.000 |  |
| noop_gravity_witness, ivmlite | bootstrap_ms | 182.721 [177.381, 192.672] | 186.614 [180.286, 197.423] | 1.021 |  |
| noop_gravity_witness, ivmlite | first_refresh_ms | 0.828 [0.803, 0.835] | 0.920 [0.901, 0.984] | 1.111 |  |
| noop_gravity_witness, ivmlite | apply_ms | 1.870 [1.842, 1.893] | 1.850 [1.835, 1.861] | 0.989 |  |
| noop_gravity_witness, ivmlite | maintain_ms | 0.462 [0.454, 0.465] | 0.438 [0.432, 0.447] | 0.948 |  |
| noop_gravity_witness, ivmlite | read_ms | 0.017 [0.016, 0.019] | 0.016 [0.016, 0.017] | 0.941 |  |
| noop_gravity_witness, ivmlite | apply_plus_maintain_ms | 2.320 [2.287, 2.343] | 2.294 [2.278, 2.306] | 0.989 |  |
| noop_gravity_witness, ivmlite | end_to_end_ms | 2.339 [2.303, 2.360] | 2.310 [2.293, 2.323] | 0.988 |  |
| noop_gravity_witness, ivmlite | database_kib | 15164.000 [15164.000, 15164.000] | 15164.000 [15164.000, 15164.000] | 1.000 |  |
| noop_gravity_witness, no_maintenance | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.003] | n/a (0/0) |  |
| noop_gravity_witness, no_maintenance | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| noop_gravity_witness, no_maintenance | apply_ms | 0.180 [0.177, 0.188] | 0.180 [0.178, 0.180] | 1.000 |  |
| noop_gravity_witness, no_maintenance | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| noop_gravity_witness, no_maintenance | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| noop_gravity_witness, no_maintenance | apply_plus_maintain_ms | 0.180 [0.177, 0.188] | 0.180 [0.178, 0.180] | 1.000 |  |
| noop_gravity_witness, no_maintenance | end_to_end_ms | 0.180 [0.177, 0.188] | 0.180 [0.178, 0.180] | 1.000 |  |
| noop_gravity_witness, no_maintenance | database_kib | 15048.000 [15048.000, 15048.000] | 15048.000 [15048.000, 15048.000] | 1.000 |  |
| noop_gravity_witness, unindexed_recompute | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| noop_gravity_witness, unindexed_recompute | first_refresh_ms | 174.181 [171.413, 179.522] | 170.270 [169.462, 173.013] | 0.978 |  |
| noop_gravity_witness, unindexed_recompute | apply_ms | 0.253 [0.241, 0.257] | 0.210 [0.204, 0.221] | 0.830 |  |
| noop_gravity_witness, unindexed_recompute | maintain_ms | 173.816 [172.915, 174.072] | 169.812 [169.721, 170.132] | 0.977 |  |
| noop_gravity_witness, unindexed_recompute | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| noop_gravity_witness, unindexed_recompute | apply_plus_maintain_ms | 174.049 [173.179, 174.347] | 170.010 [169.939, 170.348] | 0.977 |  |
| noop_gravity_witness, unindexed_recompute | end_to_end_ms | 174.049 [173.179, 174.347] | 170.010 [169.939, 170.348] | 0.977 |  |
| noop_gravity_witness, unindexed_recompute | database_kib | 15048.000 [15048.000, 15048.000] | 15048.000 [15048.000, 15048.000] | 1.000 |  |
| openwhispr_speaker_words, indexed_recompute | bootstrap_ms | 596.166 [586.526, 613.896] | 597.736 [588.552, 637.504] | 1.003 |  |
| openwhispr_speaker_words, indexed_recompute | first_refresh_ms | 502.098 [499.069, 502.555] | 499.511 [496.644, 510.386] | 0.995 |  |
| openwhispr_speaker_words, indexed_recompute | apply_ms | 0.224 [0.218, 0.245] | 0.224 [0.222, 0.230] | 1.000 |  |
| openwhispr_speaker_words, indexed_recompute | maintain_ms | 501.192 [498.331, 508.966] | 503.353 [498.928, 512.271] | 1.004 |  |
| openwhispr_speaker_words, indexed_recompute | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| openwhispr_speaker_words, indexed_recompute | apply_plus_maintain_ms | 501.413 [498.561, 509.230] | 503.569 [499.168, 512.504] | 1.004 |  |
| openwhispr_speaker_words, indexed_recompute | end_to_end_ms | 501.413 [498.561, 509.230] | 503.569 [499.168, 512.504] | 1.004 |  |
| openwhispr_speaker_words, indexed_recompute | database_kib | 130192.000 [130192.000, 130192.000] | 130192.000 [130192.000, 130192.000] | 1.000 |  |
| openwhispr_speaker_words, ivmlite | bootstrap_ms | 6463.257 [6192.212, 6722.098] | 7138.064 [6580.796, 7698.715] | 1.104 |  |
| openwhispr_speaker_words, ivmlite | first_refresh_ms | 14.537 [14.307, 14.883] | 16.247 [14.338, 25.674] | 1.118 |  |
| openwhispr_speaker_words, ivmlite | apply_ms | 1.818 [1.814, 1.820] | 1.810 [1.807, 1.844] | 0.996 |  |
| openwhispr_speaker_words, ivmlite | maintain_ms | 11.168 [11.141, 11.201] | 11.578 [11.367, 12.059] | 1.037 |  |
| openwhispr_speaker_words, ivmlite | read_ms | 0.070 [0.066, 0.075] | 0.065 [0.064, 0.080] | 0.929 |  |
| openwhispr_speaker_words, ivmlite | apply_plus_maintain_ms | 12.996 [12.965, 13.019] | 13.389 [13.192, 13.917] | 1.030 |  |
| openwhispr_speaker_words, ivmlite | end_to_end_ms | 13.066 [13.031, 13.089] | 13.460 [13.267, 14.000] | 1.030 |  |
| openwhispr_speaker_words, ivmlite | database_kib | 408048.000 [408048.000, 408048.000] | 408040.000 [408040.000, 408040.000] | 1.000 |  |
| openwhispr_speaker_words, no_maintenance | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| openwhispr_speaker_words, no_maintenance | first_refresh_ms | 0.000 [0.000, 0.001] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| openwhispr_speaker_words, no_maintenance | apply_ms | 0.091 [0.089, 0.093] | 0.090 [0.089, 0.094] | 0.989 |  |
| openwhispr_speaker_words, no_maintenance | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| openwhispr_speaker_words, no_maintenance | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| openwhispr_speaker_words, no_maintenance | apply_plus_maintain_ms | 0.091 [0.089, 0.093] | 0.090 [0.089, 0.094] | 0.989 |  |
| openwhispr_speaker_words, no_maintenance | end_to_end_ms | 0.091 [0.089, 0.093] | 0.091 [0.089, 0.094] | 1.000 |  |
| openwhispr_speaker_words, no_maintenance | database_kib | 85732.000 [85732.000, 85732.000] | 85732.000 [85732.000, 85732.000] | 1.000 |  |
| openwhispr_speaker_words, unindexed_recompute | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| openwhispr_speaker_words, unindexed_recompute | first_refresh_ms | 410.853 [403.101, 416.317] | 407.726 [405.565, 420.741] | 0.992 |  |
| openwhispr_speaker_words, unindexed_recompute | apply_ms | 0.115 [0.113, 0.117] | 0.116 [0.113, 0.118] | 1.009 |  |
| openwhispr_speaker_words, unindexed_recompute | maintain_ms | 408.500 [405.071, 413.833] | 408.349 [405.089, 421.558] | 1.000 |  |
| openwhispr_speaker_words, unindexed_recompute | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| openwhispr_speaker_words, unindexed_recompute | apply_plus_maintain_ms | 408.613 [405.182, 413.956] | 408.463 [405.205, 421.666] | 1.000 |  |
| openwhispr_speaker_words, unindexed_recompute | end_to_end_ms | 408.613 [405.182, 413.956] | 408.464 [405.205, 421.666] | 1.000 |  |
| openwhispr_speaker_words, unindexed_recompute | database_kib | 85732.000 [85732.000, 85732.000] | 85732.000 [85732.000, 85732.000] | 1.000 |  |
| phoenix_token_rollup, handwritten_trigger | bootstrap_ms | 1670.682 [1666.290, 1674.257] | 1669.941 [1664.270, 1731.623] | 1.000 |  |
| phoenix_token_rollup, handwritten_trigger | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| phoenix_token_rollup, handwritten_trigger | apply_ms | 3.040 [3.023, 3.057] | 3.037 [3.014, 3.099] | 0.999 |  |
| phoenix_token_rollup, handwritten_trigger | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| phoenix_token_rollup, handwritten_trigger | read_ms | 0.024 [0.024, 0.025] | 0.025 [0.025, 0.026] | 1.042 |  |
| phoenix_token_rollup, handwritten_trigger | apply_plus_maintain_ms | 3.040 [3.023, 3.057] | 3.037 [3.014, 3.099] | 0.999 |  |
| phoenix_token_rollup, handwritten_trigger | end_to_end_ms | 3.063 [3.048, 3.082] | 3.063 [3.041, 3.125] | 1.000 |  |
| phoenix_token_rollup, handwritten_trigger | database_kib | 157632.000 [157632.000, 157632.000] | 157632.000 [157632.000, 157632.000] | 1.000 |  |
| phoenix_token_rollup, indexed_recompute | bootstrap_ms | 1173.557 [1156.794, 1190.123] | 1181.709 [1159.550, 1219.464] | 1.007 |  |
| phoenix_token_rollup, indexed_recompute | first_refresh_ms | 1663.651 [1650.563, 1677.259] | 1695.494 [1641.112, 1789.904] | 1.019 |  |
| phoenix_token_rollup, indexed_recompute | apply_ms | 0.969 [0.948, 0.972] | 0.979 [0.942, 1.205] | 1.010 |  |
| phoenix_token_rollup, indexed_recompute | maintain_ms | 1661.781 [1656.756, 1681.732] | 1657.618 [1653.920, 1690.649] | 0.997 |  |
| phoenix_token_rollup, indexed_recompute | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| phoenix_token_rollup, indexed_recompute | apply_plus_maintain_ms | 1662.764 [1657.718, 1684.887] | 1658.586 [1654.889, 1691.873] | 0.997 |  |
| phoenix_token_rollup, indexed_recompute | end_to_end_ms | 1662.764 [1657.718, 1684.887] | 1658.586 [1654.889, 1691.873] | 0.997 |  |
| phoenix_token_rollup, indexed_recompute | database_kib | 249100.000 [249100.000, 249100.000] | 249100.000 [249100.000, 249100.000] | 1.000 |  |
| phoenix_token_rollup, ivmlite | bootstrap_ms | 16396.148 [16193.539, 16685.915] | 17664.382 [16931.931, 18415.274] | 1.077 |  |
| phoenix_token_rollup, ivmlite | first_refresh_ms | 8.323 [8.266, 8.459] | 9.151 [8.720, 9.501] | 1.099 |  |
| phoenix_token_rollup, ivmlite | apply_ms | 1.840 [1.817, 1.847] | 1.820 [1.818, 1.827] | 0.989 |  |
| phoenix_token_rollup, ivmlite | maintain_ms | 5.575 [5.571, 5.634] | 5.720 [5.702, 5.749] | 1.026 |  |
| phoenix_token_rollup, ivmlite | read_ms | 0.112 [0.109, 0.115] | 0.108 [0.107, 0.110] | 0.964 |  |
| phoenix_token_rollup, ivmlite | apply_plus_maintain_ms | 7.403 [7.386, 7.467] | 7.559 [7.525, 7.566] | 1.021 |  |
| phoenix_token_rollup, ivmlite | end_to_end_ms | 7.509 [7.487, 7.577] | 7.659 [7.629, 7.671] | 1.020 |  |
| phoenix_token_rollup, ivmlite | database_kib | 885672.000 [885672.000, 885672.000] | 885664.000 [885664.000, 885664.000] | 1.000 |  |
| phoenix_token_rollup, no_maintenance | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| phoenix_token_rollup, no_maintenance | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| phoenix_token_rollup, no_maintenance | apply_ms | 0.117 [0.115, 0.141] | 0.116 [0.116, 0.117] | 0.991 |  |
| phoenix_token_rollup, no_maintenance | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| phoenix_token_rollup, no_maintenance | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| phoenix_token_rollup, no_maintenance | apply_plus_maintain_ms | 0.117 [0.115, 0.141] | 0.116 [0.116, 0.117] | 0.991 |  |
| phoenix_token_rollup, no_maintenance | end_to_end_ms | 0.117 [0.115, 0.141] | 0.116 [0.116, 0.117] | 0.991 |  |
| phoenix_token_rollup, no_maintenance | database_kib | 157596.000 [157596.000, 157596.000] | 157596.000 [157596.000, 157596.000] | 1.000 |  |
| phoenix_token_rollup, unindexed_recompute | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| phoenix_token_rollup, unindexed_recompute | first_refresh_ms | 1692.819 [1667.262, 1729.214] | 1677.901 [1672.905, 1867.723] | 0.991 |  |
| phoenix_token_rollup, unindexed_recompute | apply_ms | 0.147 [0.146, 0.148] | 0.148 [0.147, 0.234] | 1.007 |  |
| phoenix_token_rollup, unindexed_recompute | maintain_ms | 1675.151 [1672.008, 1681.203] | 1683.530 [1677.774, 1769.086] | 1.005 |  |
| phoenix_token_rollup, unindexed_recompute | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| phoenix_token_rollup, unindexed_recompute | apply_plus_maintain_ms | 1675.297 [1672.156, 1681.353] | 1683.681 [1677.920, 1769.350] | 1.005 |  |
| phoenix_token_rollup, unindexed_recompute | end_to_end_ms | 1675.297 [1672.156, 1681.353] | 1683.681 [1677.920, 1769.350] | 1.005 |  |
| phoenix_token_rollup, unindexed_recompute | database_kib | 157596.000 [157596.000, 157596.000] | 157596.000 [157596.000, 157596.000] | 1.000 |  |
| pocketbase_author_counts, handwritten_trigger | bootstrap_ms | 3.607 [3.544, 3.640] | 3.634 [3.557, 3.659] | 1.007 |  |
| pocketbase_author_counts, handwritten_trigger | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| pocketbase_author_counts, handwritten_trigger | apply_ms | 0.219 [0.216, 0.227] | 0.221 [0.218, 0.223] | 1.009 |  |
| pocketbase_author_counts, handwritten_trigger | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| pocketbase_author_counts, handwritten_trigger | read_ms | 0.163 [0.162, 0.167] | 0.163 [0.161, 0.167] | 1.000 |  |
| pocketbase_author_counts, handwritten_trigger | apply_plus_maintain_ms | 0.219 [0.216, 0.227] | 0.221 [0.218, 0.223] | 1.009 |  |
| pocketbase_author_counts, handwritten_trigger | end_to_end_ms | 0.382 [0.379, 0.394] | 0.385 [0.380, 0.389] | 1.008 |  |
| pocketbase_author_counts, handwritten_trigger | database_kib | 572.000 [572.000, 572.000] | 572.000 [572.000, 572.000] | 1.000 |  |
| pocketbase_author_counts, indexed_recompute | bootstrap_ms | 2.411 [2.375, 2.482] | 2.392 [2.370, 2.421] | 0.992 |  |
| pocketbase_author_counts, indexed_recompute | first_refresh_ms | 0.986 [0.985, 1.020] | 0.985 [0.976, 1.018] | 0.999 |  |
| pocketbase_author_counts, indexed_recompute | apply_ms | 0.031 [0.030, 0.032] | 0.031 [0.030, 0.031] | 1.000 |  |
| pocketbase_author_counts, indexed_recompute | maintain_ms | 1.004 [0.993, 1.038] | 0.994 [0.988, 1.008] | 0.990 |  |
| pocketbase_author_counts, indexed_recompute | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| pocketbase_author_counts, indexed_recompute | apply_plus_maintain_ms | 1.037 [1.023, 1.071] | 1.025 [1.024, 1.043] | 0.988 |  |
| pocketbase_author_counts, indexed_recompute | end_to_end_ms | 1.037 [1.023, 1.071] | 1.025 [1.024, 1.043] | 0.988 |  |
| pocketbase_author_counts, indexed_recompute | database_kib | 688.000 [688.000, 688.000] | 688.000 [688.000, 688.000] | 1.000 |  |
| pocketbase_author_counts, ivmlite | bootstrap_ms | 24.682 [24.197, 26.387] | 25.078 [24.761, 27.131] | 1.016 |  |
| pocketbase_author_counts, ivmlite | first_refresh_ms | 0.612 [0.590, 0.655] | 0.604 [0.595, 0.631] | 0.987 |  |
| pocketbase_author_counts, ivmlite | apply_ms | 0.255 [0.251, 0.263] | 0.254 [0.250, 0.260] | 0.996 |  |
| pocketbase_author_counts, ivmlite | maintain_ms | 0.364 [0.351, 0.390] | 0.364 [0.358, 0.371] | 1.000 |  |
| pocketbase_author_counts, ivmlite | read_ms | 0.775 [0.771, 0.790] | 0.769 [0.763, 0.778] | 0.992 |  |
| pocketbase_author_counts, ivmlite | apply_plus_maintain_ms | 0.620 [0.603, 0.657] | 0.620 [0.610, 0.632] | 1.000 |  |
| pocketbase_author_counts, ivmlite | end_to_end_ms | 1.397 [1.380, 1.438] | 1.389 [1.375, 1.407] | 0.994 |  |
| pocketbase_author_counts, ivmlite | database_kib | 1396.000 [1396.000, 1396.000] | 1216.000 [1216.000, 1216.000] | 0.871 |  |
| pocketbase_author_counts, no_maintenance | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| pocketbase_author_counts, no_maintenance | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| pocketbase_author_counts, no_maintenance | apply_ms | 0.011 [0.011, 0.011] | 0.011 [0.011, 0.011] | 1.000 |  |
| pocketbase_author_counts, no_maintenance | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| pocketbase_author_counts, no_maintenance | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| pocketbase_author_counts, no_maintenance | apply_plus_maintain_ms | 0.011 [0.011, 0.011] | 0.011 [0.011, 0.011] | 1.000 |  |
| pocketbase_author_counts, no_maintenance | end_to_end_ms | 0.011 [0.011, 0.011] | 0.011 [0.011, 0.011] | 1.000 |  |
| pocketbase_author_counts, no_maintenance | database_kib | 344.000 [344.000, 344.000] | 344.000 [344.000, 344.000] | 1.000 |  |
| pocketbase_author_counts, unindexed_recompute | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| pocketbase_author_counts, unindexed_recompute | first_refresh_ms | 2.679 [2.614, 2.701] | 2.699 [2.676, 2.714] | 1.007 |  |
| pocketbase_author_counts, unindexed_recompute | apply_ms | 0.012 [0.012, 0.014] | 0.012 [0.012, 0.012] | 1.000 |  |
| pocketbase_author_counts, unindexed_recompute | maintain_ms | 2.670 [2.658, 2.696] | 2.691 [2.655, 2.713] | 1.008 |  |
| pocketbase_author_counts, unindexed_recompute | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| pocketbase_author_counts, unindexed_recompute | apply_plus_maintain_ms | 2.683 [2.670, 2.708] | 2.702 [2.667, 2.725] | 1.007 |  |
| pocketbase_author_counts, unindexed_recompute | end_to_end_ms | 2.683 [2.670, 2.708] | 2.702 [2.667, 2.725] | 1.007 |  |
| pocketbase_author_counts, unindexed_recompute | database_kib | 344.000 [344.000, 344.000] | 344.000 [344.000, 344.000] | 1.000 |  |
| postio_mailbox_footprint, handwritten_trigger | bootstrap_ms | 6.022 [5.956, 6.066] | 5.920 [5.855, 6.106] | 0.983 |  |
| postio_mailbox_footprint, handwritten_trigger | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| postio_mailbox_footprint, handwritten_trigger | apply_ms | 1.033 [1.026, 1.043] | 1.012 [1.000, 1.033] | 0.980 |  |
| postio_mailbox_footprint, handwritten_trigger | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| postio_mailbox_footprint, handwritten_trigger | read_ms | 0.001 [0.001, 0.001] | 0.001 [0.001, 0.001] | 1.000 |  |
| postio_mailbox_footprint, handwritten_trigger | apply_plus_maintain_ms | 1.033 [1.026, 1.043] | 1.012 [1.000, 1.033] | 0.980 |  |
| postio_mailbox_footprint, handwritten_trigger | end_to_end_ms | 1.034 [1.026, 1.044] | 1.012 [1.000, 1.033] | 0.979 |  |
| postio_mailbox_footprint, handwritten_trigger | database_kib | 1140.000 [1140.000, 1140.000] | 1140.000 [1140.000, 1140.000] | 1.000 |  |
| postio_mailbox_footprint, indexed_recompute | bootstrap_ms | 26.273 [25.930, 26.403] | 25.705 [25.481, 25.961] | 0.978 |  |
| postio_mailbox_footprint, indexed_recompute | first_refresh_ms | 1.809 [1.758, 1.854] | 1.802 [1.781, 1.814] | 0.996 |  |
| postio_mailbox_footprint, indexed_recompute | apply_ms | 0.162 [0.156, 0.169] | 0.156 [0.154, 0.161] | 0.963 |  |
| postio_mailbox_footprint, indexed_recompute | maintain_ms | 1.849 [1.787, 1.851] | 1.826 [1.791, 1.864] | 0.988 |  |
| postio_mailbox_footprint, indexed_recompute | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| postio_mailbox_footprint, indexed_recompute | apply_plus_maintain_ms | 2.012 [1.952, 2.020] | 1.987 [1.944, 2.036] | 0.988 |  |
| postio_mailbox_footprint, indexed_recompute | end_to_end_ms | 2.012 [1.952, 2.020] | 1.987 [1.944, 2.036] | 0.988 |  |
| postio_mailbox_footprint, indexed_recompute | database_kib | 2284.000 [2284.000, 2284.000] | 2284.000 [2284.000, 2284.000] | 1.000 |  |
| postio_mailbox_footprint, ivmlite | bootstrap_ms | 37.497 [36.249, 40.149] | 39.677 [37.616, 43.041] | 1.058 |  |
| postio_mailbox_footprint, ivmlite | first_refresh_ms | 0.401 [0.386, 0.443] | 0.424 [0.411, 0.442] | 1.057 |  |
| postio_mailbox_footprint, ivmlite | apply_ms | 1.571 [1.564, 1.585] | 1.531 [1.529, 1.573] | 0.975 |  |
| postio_mailbox_footprint, ivmlite | maintain_ms | 0.247 [0.243, 0.250] | 0.238 [0.237, 0.244] | 0.964 |  |
| postio_mailbox_footprint, ivmlite | read_ms | 0.008 [0.008, 0.009] | 0.008 [0.007, 0.008] | 1.000 |  |
| postio_mailbox_footprint, ivmlite | apply_plus_maintain_ms | 1.815 [1.813, 1.841] | 1.771 [1.767, 1.828] | 0.976 |  |
| postio_mailbox_footprint, ivmlite | end_to_end_ms | 1.823 [1.820, 1.850] | 1.779 [1.774, 1.836] | 0.976 |  |
| postio_mailbox_footprint, ivmlite | database_kib | 1236.000 [1236.000, 1236.000] | 1236.000 [1236.000, 1236.000] | 1.000 |  |
| postio_mailbox_footprint, no_maintenance | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| postio_mailbox_footprint, no_maintenance | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| postio_mailbox_footprint, no_maintenance | apply_ms | 0.049 [0.049, 0.052] | 0.049 [0.048, 0.049] | 1.000 |  |
| postio_mailbox_footprint, no_maintenance | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| postio_mailbox_footprint, no_maintenance | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| postio_mailbox_footprint, no_maintenance | apply_plus_maintain_ms | 0.049 [0.049, 0.052] | 0.049 [0.049, 0.049] | 1.000 |  |
| postio_mailbox_footprint, no_maintenance | end_to_end_ms | 0.049 [0.049, 0.052] | 0.049 [0.049, 0.049] | 1.000 |  |
| postio_mailbox_footprint, no_maintenance | database_kib | 1136.000 [1136.000, 1136.000] | 1136.000 [1136.000, 1136.000] | 1.000 |  |
| postio_mailbox_footprint, unindexed_recompute | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| postio_mailbox_footprint, unindexed_recompute | first_refresh_ms | 5.785 [5.686, 5.799] | 5.713 [5.678, 5.732] | 0.988 |  |
| postio_mailbox_footprint, unindexed_recompute | apply_ms | 0.049 [0.049, 0.050] | 0.049 [0.049, 0.051] | 1.000 |  |
| postio_mailbox_footprint, unindexed_recompute | maintain_ms | 5.760 [5.674, 5.784] | 5.674 [5.668, 5.745] | 0.985 |  |
| postio_mailbox_footprint, unindexed_recompute | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| postio_mailbox_footprint, unindexed_recompute | apply_plus_maintain_ms | 5.809 [5.724, 5.835] | 5.725 [5.717, 5.797] | 0.986 |  |
| postio_mailbox_footprint, unindexed_recompute | end_to_end_ms | 5.809 [5.724, 5.835] | 5.725 [5.717, 5.797] | 0.986 |  |
| postio_mailbox_footprint, unindexed_recompute | database_kib | 1136.000 [1136.000, 1136.000] | 1136.000 [1136.000, 1136.000] | 1.000 |  |
| zcash_transparent_balance, indexed_recompute | bootstrap_ms | 217.567 [215.916, 219.470] | 217.716 [210.908, 223.022] | 1.001 |  |
| zcash_transparent_balance, indexed_recompute | first_refresh_ms | 22.697 [22.210, 22.844] | 21.977 [21.965, 22.121] | 0.968 |  |
| zcash_transparent_balance, indexed_recompute | apply_ms | 0.948 [0.822, 1.040] | 0.788 [0.781, 0.797] | 0.831 |  |
| zcash_transparent_balance, indexed_recompute | maintain_ms | 22.417 [22.323, 22.973] | 21.978 [21.945, 22.012] | 0.980 |  |
| zcash_transparent_balance, indexed_recompute | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| zcash_transparent_balance, indexed_recompute | apply_plus_maintain_ms | 23.361 [23.131, 23.977] | 22.786 [22.740, 22.821] | 0.975 |  |
| zcash_transparent_balance, indexed_recompute | end_to_end_ms | 23.361 [23.131, 23.977] | 22.786 [22.740, 22.821] | 0.975 |  |
| zcash_transparent_balance, indexed_recompute | database_kib | 25772.000 [25772.000, 25772.000] | 25772.000 [25772.000, 25772.000] | 1.000 |  |
| zcash_transparent_balance, ivmlite | bootstrap_ms | 518.228 [507.300, 546.781] | 528.219 [512.864, 547.632] | 1.019 |  |
| zcash_transparent_balance, ivmlite | first_refresh_ms | 3.004 [2.957, 3.230] | 2.947 [2.892, 3.041] | 0.981 |  |
| zcash_transparent_balance, ivmlite | apply_ms | 4.525 [4.471, 4.719] | 4.446 [4.414, 4.472] | 0.983 |  |
| zcash_transparent_balance, ivmlite | maintain_ms | 2.375 [2.352, 2.515] | 2.407 [2.395, 2.442] | 1.013 |  |
| zcash_transparent_balance, ivmlite | read_ms | 0.048 [0.047, 0.057] | 0.047 [0.046, 0.048] | 0.979 |  |
| zcash_transparent_balance, ivmlite | apply_plus_maintain_ms | 6.909 [6.832, 7.198] | 6.855 [6.818, 6.946] | 0.992 |  |
| zcash_transparent_balance, ivmlite | end_to_end_ms | 6.958 [6.879, 7.252] | 6.902 [6.864, 6.996] | 0.992 |  |
| zcash_transparent_balance, ivmlite | database_kib | 12540.000 [12540.000, 12540.000] | 12544.000 [12544.000, 12544.000] | 1.000 |  |
| zcash_transparent_balance, no_maintenance | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| zcash_transparent_balance, no_maintenance | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| zcash_transparent_balance, no_maintenance | apply_ms | 0.159 [0.154, 0.164] | 0.159 [0.158, 0.159] | 1.000 |  |
| zcash_transparent_balance, no_maintenance | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| zcash_transparent_balance, no_maintenance | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| zcash_transparent_balance, no_maintenance | apply_plus_maintain_ms | 0.159 [0.154, 0.164] | 0.159 [0.158, 0.159] | 1.000 |  |
| zcash_transparent_balance, no_maintenance | end_to_end_ms | 0.159 [0.154, 0.164] | 0.159 [0.158, 0.159] | 1.000 |  |
| zcash_transparent_balance, no_maintenance | database_kib | 12328.000 [12328.000, 12328.000] | 12328.000 [12328.000, 12328.000] | 1.000 |  |
| zcash_transparent_balance, unindexed_recompute | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| zcash_transparent_balance, unindexed_recompute | first_refresh_ms | 116.465 [115.315, 118.186] | 114.006 [113.330, 119.116] | 0.979 |  |
| zcash_transparent_balance, unindexed_recompute | apply_ms | 0.174 [0.172, 0.178] | 0.164 [0.162, 0.167] | 0.943 |  |
| zcash_transparent_balance, unindexed_recompute | maintain_ms | 116.624 [115.896, 119.730] | 116.958 [114.507, 121.854] | 1.003 |  |
| zcash_transparent_balance, unindexed_recompute | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| zcash_transparent_balance, unindexed_recompute | apply_plus_maintain_ms | 116.798 [116.070, 119.935] | 117.127 [114.671, 122.021] | 1.003 |  |
| zcash_transparent_balance, unindexed_recompute | end_to_end_ms | 116.798 [116.070, 119.935] | 117.127 [114.671, 122.021] | 1.003 |  |
| zcash_transparent_balance, unindexed_recompute | database_kib | 12328.000 [12328.000, 12328.000] | 12328.000 [12328.000, 12328.000] | 1.000 |  |
| zenod_usage_totals, handwritten_trigger | bootstrap_ms | 764.759 [754.133, 788.306] | 774.817 [747.747, 781.247] | 1.013 |  |
| zenod_usage_totals, handwritten_trigger | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.003] | n/a (0/0) |  |
| zenod_usage_totals, handwritten_trigger | apply_ms | 1.585 [1.546, 1.674] | 1.548 [1.523, 1.565] | 0.977 |  |
| zenod_usage_totals, handwritten_trigger | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| zenod_usage_totals, handwritten_trigger | read_ms | 0.011 [0.010, 0.012] | 0.011 [0.011, 0.011] | 1.000 |  |
| zenod_usage_totals, handwritten_trigger | apply_plus_maintain_ms | 1.585 [1.546, 1.674] | 1.548 [1.523, 1.565] | 0.977 |  |
| zenod_usage_totals, handwritten_trigger | end_to_end_ms | 1.596 [1.557, 1.686] | 1.558 [1.534, 1.576] | 0.976 |  |
| zenod_usage_totals, handwritten_trigger | database_kib | 32952.000 [32952.000, 32952.000] | 32952.000 [32952.000, 32952.000] | 1.000 |  |
| zenod_usage_totals, indexed_recompute | bootstrap_ms | 921.388 [919.411, 950.217] | 910.249 [895.360, 933.611] | 0.988 |  |
| zenod_usage_totals, indexed_recompute | first_refresh_ms | 97.384 [96.622, 99.034] | 95.337 [95.181, 95.813] | 0.979 |  |
| zenod_usage_totals, indexed_recompute | apply_ms | 0.597 [0.579, 0.631] | 0.508 [0.475, 0.604] | 0.851 |  |
| zenod_usage_totals, indexed_recompute | maintain_ms | 97.825 [97.512, 98.640] | 94.988 [94.788, 95.656] | 0.971 |  |
| zenod_usage_totals, indexed_recompute | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| zenod_usage_totals, indexed_recompute | apply_plus_maintain_ms | 98.539 [98.128, 99.380] | 95.504 [95.283, 96.330] | 0.969 |  |
| zenod_usage_totals, indexed_recompute | end_to_end_ms | 98.539 [98.128, 99.380] | 95.504 [95.283, 96.330] | 0.969 |  |
| zenod_usage_totals, indexed_recompute | database_kib | 66580.000 [66580.000, 66580.000] | 66580.000 [66580.000, 66580.000] | 1.000 |  |
| zenod_usage_totals, ivmlite | bootstrap_ms | 968.961 [948.405, 1002.358] | 997.126 [923.489, 1066.830] | 1.029 |  |
| zenod_usage_totals, ivmlite | first_refresh_ms | 1.566 [1.500, 1.626] | 1.334 [1.303, 1.711] | 0.852 |  |
| zenod_usage_totals, ivmlite | apply_ms | 1.646 [1.618, 1.651] | 1.610 [1.600, 1.662] | 0.978 |  |
| zenod_usage_totals, ivmlite | maintain_ms | 1.077 [1.064, 1.086] | 1.071 [1.056, 1.085] | 0.994 |  |
| zenod_usage_totals, ivmlite | read_ms | 0.060 [0.059, 0.061] | 0.056 [0.055, 0.060] | 0.933 |  |
| zenod_usage_totals, ivmlite | apply_plus_maintain_ms | 2.728 [2.684, 2.737] | 2.680 [2.651, 2.740] | 0.982 |  |
| zenod_usage_totals, ivmlite | end_to_end_ms | 2.786 [2.745, 2.796] | 2.739 [2.706, 2.801] | 0.983 |  |
| zenod_usage_totals, ivmlite | database_kib | 33104.000 [33104.000, 33104.000] | 33092.000 [33092.000, 33092.000] | 1.000 |  |
| zenod_usage_totals, no_maintenance | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| zenod_usage_totals, no_maintenance | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| zenod_usage_totals, no_maintenance | apply_ms | 0.065 [0.064, 0.070] | 0.066 [0.064, 0.069] | 1.015 |  |
| zenod_usage_totals, no_maintenance | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| zenod_usage_totals, no_maintenance | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| zenod_usage_totals, no_maintenance | apply_plus_maintain_ms | 0.065 [0.064, 0.070] | 0.066 [0.064, 0.069] | 1.015 |  |
| zenod_usage_totals, no_maintenance | end_to_end_ms | 0.065 [0.064, 0.070] | 0.066 [0.064, 0.069] | 1.015 |  |
| zenod_usage_totals, no_maintenance | database_kib | 32936.000 [32936.000, 32936.000] | 32936.000 [32936.000, 32936.000] | 1.000 |  |
| zenod_usage_totals, unindexed_recompute | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| zenod_usage_totals, unindexed_recompute | first_refresh_ms | 776.210 [758.901, 805.862] | 762.264 [749.818, 776.234] | 0.982 |  |
| zenod_usage_totals, unindexed_recompute | apply_ms | 0.088 [0.084, 0.094] | 0.082 [0.082, 0.084] | 0.932 |  |
| zenod_usage_totals, unindexed_recompute | maintain_ms | 765.482 [760.669, 788.928] | 765.097 [750.388, 771.080] | 0.999 |  |
| zenod_usage_totals, unindexed_recompute | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| zenod_usage_totals, unindexed_recompute | apply_plus_maintain_ms | 765.568 [760.756, 789.015] | 765.181 [750.477, 771.161] | 0.999 |  |
| zenod_usage_totals, unindexed_recompute | end_to_end_ms | 765.568 [760.756, 789.015] | 765.181 [750.477, 771.161] | 0.999 |  |
| zenod_usage_totals, unindexed_recompute | database_kib | 32936.000 [32936.000, 32936.000] | 32936.000 [32936.000, 32936.000] | 1.000 |  |

CHANGED SHAPE: FluxFlow and Kener

Before uses each original integer query/harness; after uses non-STRICT REAL SUM (FluxFlow) or AVG/MIN/MAX/COUNT (Kener). These shapes are outside the integer budget. Their ratio is not an engine improvement or a budget verdict.

| case, mode | metric | before | after | ratio | flag |
|---|---|---:|---:|---:|---|
| fluxflow_grouped_flow_rollup, handwritten_trigger | bootstrap_ms | 1175.378 [1170.626, 1201.700] | 1200.204 [1191.149, 1231.629] | 1.021 |  |
| fluxflow_grouped_flow_rollup, handwritten_trigger | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| fluxflow_grouped_flow_rollup, handwritten_trigger | apply_ms | 1.275 [1.269, 1.285] | 1.286 [1.276, 1.299] | 1.009 |  |
| fluxflow_grouped_flow_rollup, handwritten_trigger | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| fluxflow_grouped_flow_rollup, handwritten_trigger | read_ms | 0.458 [0.455, 0.459] | 0.475 [0.474, 0.475] | 1.037 |  |
| fluxflow_grouped_flow_rollup, handwritten_trigger | apply_plus_maintain_ms | 1.275 [1.269, 1.285] | 1.286 [1.276, 1.299] | 1.009 |  |
| fluxflow_grouped_flow_rollup, handwritten_trigger | end_to_end_ms | 1.735 [1.726, 1.745] | 1.760 [1.754, 1.775] | 1.014 |  |
| fluxflow_grouped_flow_rollup, handwritten_trigger | database_kib | 61144.000 [61144.000, 61144.000] | 77544.000 [77544.000, 77544.000] | 1.268 |  |
| fluxflow_grouped_flow_rollup, indexed_recompute | bootstrap_ms | 1258.141 [1248.100, 1278.183] | 1297.668 [1279.306, 1335.844] | 1.031 |  |
| fluxflow_grouped_flow_rollup, indexed_recompute | first_refresh_ms | 130.261 [130.082, 131.676] | 134.839 [134.469, 140.226] | 1.035 |  |
| fluxflow_grouped_flow_rollup, indexed_recompute | apply_ms | 0.996 [0.979, 1.014] | 1.168 [1.137, 1.254] | 1.173 |  |
| fluxflow_grouped_flow_rollup, indexed_recompute | maintain_ms | 130.080 [129.751, 130.362] | 134.743 [134.605, 136.052] | 1.036 |  |
| fluxflow_grouped_flow_rollup, indexed_recompute | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| fluxflow_grouped_flow_rollup, indexed_recompute | apply_plus_maintain_ms | 131.079 [130.755, 131.233] | 135.890 [135.769, 137.335] | 1.037 |  |
| fluxflow_grouped_flow_rollup, indexed_recompute | end_to_end_ms | 131.079 [130.755, 131.233] | 135.890 [135.769, 137.335] | 1.037 |  |
| fluxflow_grouped_flow_rollup, indexed_recompute | database_kib | 121668.000 [121668.000, 121668.000] | 141028.000 [141028.000, 141028.000] | 1.159 |  |
| fluxflow_grouped_flow_rollup, ivmlite | bootstrap_ms | 2321.588 [2282.168, 2483.217] | 2834.756 [2640.757, 3013.083] | 1.221 |  |
| fluxflow_grouped_flow_rollup, ivmlite | first_refresh_ms | 2.454 [2.424, 2.725] | 3.989 [3.720, 6.244] | 1.626 |  |
| fluxflow_grouped_flow_rollup, ivmlite | apply_ms | 3.862 [3.854, 4.018] | 3.921 [3.893, 4.082] | 1.015 |  |
| fluxflow_grouped_flow_rollup, ivmlite | maintain_ms | 1.739 [1.720, 1.839] | 2.128 [2.097, 2.288] | 1.224 |  |
| fluxflow_grouped_flow_rollup, ivmlite | read_ms | 2.300 [2.282, 2.339] | 2.301 [2.295, 2.354] | 1.000 |  |
| fluxflow_grouped_flow_rollup, ivmlite | apply_plus_maintain_ms | 5.619 [5.584, 5.901] | 6.052 [6.015, 6.368] | 1.077 |  |
| fluxflow_grouped_flow_rollup, ivmlite | end_to_end_ms | 7.914 [7.874, 8.267] | 8.347 [8.304, 8.710] | 1.055 |  |
| fluxflow_grouped_flow_rollup, ivmlite | database_kib | 63272.000 [63272.000, 63272.000] | 83872.000 [83872.000, 83872.000] | 1.326 |  |
| fluxflow_grouped_flow_rollup, no_maintenance | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| fluxflow_grouped_flow_rollup, no_maintenance | first_refresh_ms | 0.000 [0.000, 0.001] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| fluxflow_grouped_flow_rollup, no_maintenance | apply_ms | 0.194 [0.192, 0.196] | 0.197 [0.193, 0.201] | 1.015 |  |
| fluxflow_grouped_flow_rollup, no_maintenance | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| fluxflow_grouped_flow_rollup, no_maintenance | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| fluxflow_grouped_flow_rollup, no_maintenance | apply_plus_maintain_ms | 0.194 [0.192, 0.196] | 0.198 [0.193, 0.201] | 1.021 |  |
| fluxflow_grouped_flow_rollup, no_maintenance | end_to_end_ms | 0.194 [0.192, 0.196] | 0.198 [0.193, 0.201] | 1.021 |  |
| fluxflow_grouped_flow_rollup, no_maintenance | database_kib | 60840.000 [60840.000, 60840.000] | 77224.000 [77224.000, 77224.000] | 1.269 |  |
| fluxflow_grouped_flow_rollup, unindexed_recompute | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| fluxflow_grouped_flow_rollup, unindexed_recompute | first_refresh_ms | 1179.780 [1175.689, 1192.282] | 1217.760 [1206.021, 1237.943] | 1.032 |  |
| fluxflow_grouped_flow_rollup, unindexed_recompute | apply_ms | 0.212 [0.209, 0.218] | 0.222 [0.215, 0.232] | 1.047 |  |
| fluxflow_grouped_flow_rollup, unindexed_recompute | maintain_ms | 1176.925 [1176.798, 1185.051] | 1210.346 [1201.575, 1233.281] | 1.028 |  |
| fluxflow_grouped_flow_rollup, unindexed_recompute | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| fluxflow_grouped_flow_rollup, unindexed_recompute | apply_plus_maintain_ms | 1177.133 [1177.012, 1185.280] | 1210.569 [1201.801, 1233.507] | 1.028 |  |
| fluxflow_grouped_flow_rollup, unindexed_recompute | end_to_end_ms | 1177.133 [1177.012, 1185.280] | 1210.569 [1201.801, 1233.507] | 1.028 |  |
| fluxflow_grouped_flow_rollup, unindexed_recompute | database_kib | 60840.000 [60840.000, 60840.000] | 77224.000 [77224.000, 77224.000] | 1.269 |  |
| kener_quarter_hour_rollup, indexed_recompute | bootstrap_ms | 1635.657 [1631.255, 1648.088] | 1637.267 [1628.107, 1700.497] | 1.001 |  |
| kener_quarter_hour_rollup, indexed_recompute | first_refresh_ms | 340.346 [338.623, 342.107] | 572.818 [570.149, 576.866] | 1.683 |  |
| kener_quarter_hour_rollup, indexed_recompute | apply_ms | 1.096 [1.085, 1.108] | 1.207 [1.197, 1.229] | 1.101 |  |
| kener_quarter_hour_rollup, indexed_recompute | maintain_ms | 339.962 [339.069, 340.367] | 571.278 [570.310, 574.859] | 1.680 |  |
| kener_quarter_hour_rollup, indexed_recompute | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| kener_quarter_hour_rollup, indexed_recompute | apply_plus_maintain_ms | 341.124 [340.192, 341.407] | 572.612 [571.511, 576.074] | 1.679 |  |
| kener_quarter_hour_rollup, indexed_recompute | end_to_end_ms | 341.124 [340.192, 341.407] | 572.612 [571.511, 576.074] | 1.679 |  |
| kener_quarter_hour_rollup, indexed_recompute | database_kib | 200596.000 [200596.000, 200596.000] | 237728.000 [237728.000, 237728.000] | 1.185 |  |
| kener_quarter_hour_rollup, ivmlite | bootstrap_ms | 6016.495 [5900.298, 6221.211] | 30638.833 [29687.721, 31928.525] | 5.092 |  |
| kener_quarter_hour_rollup, ivmlite | first_refresh_ms | 7.091 [6.711, 12.943] | 17.006 [16.310, 20.899] | 2.398 |  |
| kener_quarter_hour_rollup, ivmlite | apply_ms | 3.974 [3.956, 3.994] | 4.057 [4.042, 4.072] | 1.021 |  |
| kener_quarter_hour_rollup, ivmlite | maintain_ms | 1.651 [1.643, 1.663] | 7.010 [6.988, 8.574] | 4.246 |  |
| kener_quarter_hour_rollup, ivmlite | read_ms | 129.841 [129.238, 130.567] | 155.472 [155.102, 159.344] | 1.197 |  |
| kener_quarter_hour_rollup, ivmlite | apply_plus_maintain_ms | 6.158 [6.103, 6.191] | 11.072 [11.033, 12.648] | 1.798 |  |
| kener_quarter_hour_rollup, ivmlite | end_to_end_ms | 135.932 [135.246, 136.705] | 166.546 [166.160, 171.359] | 1.225 |  |
| kener_quarter_hour_rollup, ivmlite | database_kib | 255436.000 [255436.000, 255436.000] | 1760784.000 [1760784.000, 1760784.000] | 6.893 |  |
| kener_quarter_hour_rollup, no_maintenance | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| kener_quarter_hour_rollup, no_maintenance | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| kener_quarter_hour_rollup, no_maintenance | apply_ms | 0.161 [0.160, 0.162] | 0.167 [0.162, 0.170] | 1.037 |  |
| kener_quarter_hour_rollup, no_maintenance | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| kener_quarter_hour_rollup, no_maintenance | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| kener_quarter_hour_rollup, no_maintenance | apply_plus_maintain_ms | 0.161 [0.160, 0.162] | 0.167 [0.162, 0.170] | 1.037 |  |
| kener_quarter_hour_rollup, no_maintenance | end_to_end_ms | 0.161 [0.161, 0.162] | 0.167 [0.162, 0.170] | 1.037 |  |
| kener_quarter_hour_rollup, no_maintenance | database_kib | 100940.000 [100940.000, 100940.000] | 119500.000 [119500.000, 119500.000] | 1.184 |  |
| kener_quarter_hour_rollup, unindexed_recompute | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| kener_quarter_hour_rollup, unindexed_recompute | first_refresh_ms | 1652.193 [1639.783, 1667.552] | 1912.466 [1895.986, 1922.866] | 1.158 |  |
| kener_quarter_hour_rollup, unindexed_recompute | apply_ms | 0.186 [0.182, 0.190] | 0.192 [0.189, 0.209] | 1.032 |  |
| kener_quarter_hour_rollup, unindexed_recompute | maintain_ms | 1644.106 [1638.330, 1651.092] | 1908.265 [1901.148, 1914.990] | 1.161 |  |
| kener_quarter_hour_rollup, unindexed_recompute | read_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| kener_quarter_hour_rollup, unindexed_recompute | apply_plus_maintain_ms | 1644.296 [1638.514, 1651.408] | 1908.596 [1901.472, 1915.313] | 1.161 |  |
| kener_quarter_hour_rollup, unindexed_recompute | end_to_end_ms | 1644.296 [1638.514, 1651.408] | 1908.596 [1901.472, 1915.313] | 1.161 |  |
| kener_quarter_hour_rollup, unindexed_recompute | database_kib | 100940.000 [100940.000, 100940.000] | 119500.000 [119500.000, 119500.000] | 1.184 |  |

Write amplification (one capture per cell; no repeat ranges or integer maintenance-budget claim)

| views/op/recursive | metric | before | after | after/before |
|---|---|---:|---:|---:|
| 10/delete/false | apply_ms | 5.798 | 4.933 | 0.851 |
| 10/delete/false | us_per_row | 5.798 | 4.933 | 0.851 |
| 10/delete/false | extra_us_per_row | 3.770 | 3.156 | 0.837 |
| 10/delete/true | apply_ms | 5.393 | 5.400 | 1.001 |
| 10/delete/true | us_per_row | 5.393 | 5.400 | 1.001 |
| 10/delete/true | extra_us_per_row | 3.499 | 3.700 | 1.057 |
| 10/insert/false | apply_ms | 15.614 | 15.408 | 0.987 |
| 10/insert/false | us_per_row | 15.614 | 15.408 | 0.987 |
| 10/insert/false | extra_us_per_row | 14.433 | 14.254 | 0.988 |
| 10/insert/true | apply_ms | 15.822 | 15.432 | 0.975 |
| 10/insert/true | us_per_row | 15.822 | 15.432 | 0.975 |
| 10/insert/true | extra_us_per_row | 14.694 | 14.269 | 0.971 |
| 10/replace_rowid/false | apply_ms | 22.330 | 23.326 | 1.045 |
| 10/replace_rowid/false | us_per_row | 22.330 | 23.326 | 1.045 |
| 10/replace_rowid/false | extra_us_per_row | 16.988 | 19.020 | 1.120 |
| 10/replace_rowid/true | apply_ms | 25.073 | 21.730 | 0.867 |
| 10/replace_rowid/true | us_per_row | 25.073 | 21.730 | 0.867 |
| 10/replace_rowid/true | extra_us_per_row | 20.475 | 17.522 | 0.856 |
| 10/replace_two/false | apply_ms | 27.945 | 26.360 | 0.943 |
| 10/replace_two/false | us_per_row | 27.945 | 26.360 | 0.943 |
| 10/replace_two/false | extra_us_per_row | 21.736 | 20.367 | 0.937 |
| 10/replace_two/true | apply_ms | 26.846 | 25.932 | 0.966 |
| 10/replace_two/true | us_per_row | 26.846 | 25.932 | 0.966 |
| 10/replace_two/true | extra_us_per_row | 20.319 | 20.021 | 0.985 |
| 10/replace_unique/false | apply_ms | 22.297 | 22.425 | 1.006 |
| 10/replace_unique/false | us_per_row | 22.297 | 22.425 | 1.006 |
| 10/replace_unique/false | extra_us_per_row | 17.813 | 18.100 | 1.016 |
| 10/replace_unique/true | apply_ms | 22.413 | 21.358 | 0.953 |
| 10/replace_unique/true | us_per_row | 22.413 | 21.358 | 0.953 |
| 10/replace_unique/true | extra_us_per_row | 18.154 | 16.974 | 0.935 |
| 10/update/false | apply_ms | 16.146 | 17.022 | 1.054 |
| 10/update/false | us_per_row | 16.146 | 17.022 | 1.054 |
| 10/update/false | extra_us_per_row | 15.081 | 15.973 | 1.059 |
| 10/update/true | apply_ms | 16.306 | 18.344 | 1.125 |
| 10/update/true | us_per_row | 16.306 | 18.344 | 1.125 |
| 10/update/true | extra_us_per_row | 15.420 | 17.204 | 1.116 |
| 200/delete/false | apply_ms | 5.715 | 5.996 | 1.049 |
| 200/delete/false | us_per_row | 5.715 | 5.996 | 1.049 |
| 200/delete/false | extra_us_per_row | 3.558 | 3.838 | 1.079 |
| 200/delete/true | apply_ms | 5.406 | 4.986 | 0.922 |
| 200/delete/true | us_per_row | 5.406 | 4.986 | 0.922 |
| 200/delete/true | extra_us_per_row | 3.481 | 3.026 | 0.869 |
| 200/insert/false | apply_ms | 62.861 | 62.929 | 1.001 |
| 200/insert/false | us_per_row | 62.861 | 62.929 | 1.001 |
| 200/insert/false | extra_us_per_row | 61.697 | 61.770 | 1.001 |
| 200/insert/true | apply_ms | 60.995 | 62.138 | 1.019 |
| 200/insert/true | us_per_row | 60.995 | 62.138 | 1.019 |
| 200/insert/true | extra_us_per_row | 59.832 | 60.979 | 1.019 |
| 200/replace_rowid/false | apply_ms | 68.235 | 69.465 | 1.018 |
| 200/replace_rowid/false | us_per_row | 68.235 | 69.465 | 1.018 |
| 200/replace_rowid/false | extra_us_per_row | 63.858 | 65.063 | 1.019 |
| 200/replace_rowid/true | apply_ms | 77.043 | 71.204 | 0.924 |
| 200/replace_rowid/true | us_per_row | 77.043 | 71.204 | 0.924 |
| 200/replace_rowid/true | extra_us_per_row | 71.737 | 66.663 | 0.929 |
| 200/replace_two/false | apply_ms | 81.813 | 73.418 | 0.897 |
| 200/replace_two/false | us_per_row | 81.813 | 73.418 | 0.897 |
| 200/replace_two/false | extra_us_per_row | 73.744 | 64.284 | 0.872 |
| 200/replace_two/true | apply_ms | 73.887 | 74.337 | 1.006 |
| 200/replace_two/true | us_per_row | 73.887 | 74.337 | 1.006 |
| 200/replace_two/true | extra_us_per_row | 66.898 | 66.920 | 1.000 |
| 200/replace_unique/false | apply_ms | 74.554 | 68.956 | 0.925 |
| 200/replace_unique/false | us_per_row | 74.554 | 68.956 | 0.925 |
| 200/replace_unique/false | extra_us_per_row | 69.455 | 64.031 | 0.922 |
| 200/replace_unique/true | apply_ms | 70.194 | 68.662 | 0.978 |
| 200/replace_unique/true | us_per_row | 70.194 | 68.662 | 0.978 |
| 200/replace_unique/true | extra_us_per_row | 63.915 | 64.330 | 1.006 |
| 200/update/false | apply_ms | 61.570 | 63.031 | 1.024 |
| 200/update/false | us_per_row | 61.570 | 63.031 | 1.024 |
| 200/update/false | extra_us_per_row | 60.400 | 62.148 | 1.029 |
| 200/update/true | apply_ms | 63.176 | 62.752 | 0.993 |
| 200/update/true | us_per_row | 63.176 | 62.752 | 0.993 |
| 200/update/true | extra_us_per_row | 62.269 | 61.860 | 0.993 |

Original integer checks flagged: 2 metric/cell comparisons above 1.05. Original flags remain visible above.

Confirmation rerun: 40 rows; five fresh runs per label/engine, same selected cells, each frozen build's own clean harness/library. This follow-up preserves the original captures and the 1.05 threshold.

| cell (views/base/batch/groups), engine | metric | before | after | ratio | flag |
|---|---|---:|---:|---:|---|
| 1/10000/100/1000/hand_written_trigger | bootstrap_ms | 2.410 [2.393, 2.569] | 2.497 [2.435, 2.580] | 1.036 |  |
| 1/10000/100/1000/hand_written_trigger | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 1/10000/100/1000/hand_written_trigger | apply_ms | 0.170 [0.168, 0.174] | 0.179 [0.169, 0.182] | 1.053 |  |
| 1/10000/100/1000/hand_written_trigger | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 1/10000/100/1000/hand_written_trigger | apply_plus_maintain_ms | 0.170 [0.168, 0.174] | 0.179 [0.169, 0.182] | 1.053 | >1.05 |
| 1/10000/100/1000/ivmlite | bootstrap_ms | 12.203 [11.943, 14.515] | 13.538 [13.082, 14.824] | 1.109 |  |
| 1/10000/100/1000/ivmlite | first_refresh_ms | 1.126 [1.067, 1.233] | 1.128 [1.103, 1.325] | 1.002 |  |
| 1/10000/100/1000/ivmlite | apply_ms | 0.541 [0.533, 0.552] | 0.559 [0.543, 0.589] | 1.033 |  |
| 1/10000/100/1000/ivmlite | maintain_ms | 0.838 [0.835, 0.848] | 0.851 [0.824, 0.887] | 1.016 |  |
| 1/10000/100/1000/ivmlite | apply_plus_maintain_ms | 1.360 [1.350, 1.386] | 1.417 [1.371, 1.480] | 1.042 |  |
| 1/10000/100/1000/naive_recompute | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 1/10000/100/1000/naive_recompute | first_refresh_ms | 2.154 [2.143, 2.330] | 2.233 [2.223, 2.389] | 1.037 |  |
| 1/10000/100/1000/naive_recompute | apply_ms | 0.035 [0.035, 0.039] | 0.037 [0.035, 0.040] | 1.057 |  |
| 1/10000/100/1000/naive_recompute | maintain_ms | 2.184 [2.168, 2.235] | 2.243 [2.222, 2.304] | 1.027 |  |
| 1/10000/100/1000/naive_recompute | apply_plus_maintain_ms | 2.221 [2.202, 2.271] | 2.284 [2.260, 2.339] | 1.028 |  |
| 1/10000/100/1000/no_maintenance | bootstrap_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 1/10000/100/1000/no_maintenance | first_refresh_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 1/10000/100/1000/no_maintenance | apply_ms | 0.034 [0.034, 0.036] | 0.034 [0.034, 0.035] | 1.000 |  |
| 1/10000/100/1000/no_maintenance | maintain_ms | 0.000 [0.000, 0.000] | 0.000 [0.000, 0.000] | n/a (0/0) |  |
| 1/10000/100/1000/no_maintenance | apply_plus_maintain_ms | 0.034 [0.034, 0.036] | 0.034 [0.034, 0.035] | 1.000 |  |

Original confirmation flags reproduced: 0/2. New rerun flags: 1; these remain reported, and are not evidence of a persistent regression. No further rerun was selected to seek a pass.
The originally flagged confirmation comparisons did not reproduce above 1.05.

Scoped build provenance

| scope | label | extension | harness | SQLite |
|---|---|---|---|---|
| ablation | before | b647bcba9fa5f8eb583b162f87f7dfd79a10f7fa | aba0fd730ddd52e2a3700ff6381a17c40e2bdf29 | 3.53.2 |
| ablation | after | 84e4c07c2ebfc9db8b6bafb57378627414df5524 | aba0fd730ddd52e2a3700ff6381a17c40e2bdf29 | 3.53.2 |
| confirm | before | b647bcba9fa5f8eb583b162f87f7dfd79a10f7fa | b647bcba9fa5f8eb583b162f87f7dfd79a10f7fa | 3.53.2 |
| confirm | after | 84e4c07c2ebfc9db8b6bafb57378627414df5524 | 84e4c07c2ebfc9db8b6bafb57378627414df5524 | 3.53.2 |
| write_amp | before | b647bcba9fa5f8eb583b162f87f7dfd79a10f7fa | aba0fd730ddd52e2a3700ff6381a17c40e2bdf29 | 3.53.2 |
| write_amp | after | 84e4c07c2ebfc9db8b6bafb57378627414df5524 | aba0fd730ddd52e2a3700ff6381a17c40e2bdf29 | 3.53.2 |
| nicti_facet_counts | before | b647bcba9fa5f8eb583b162f87f7dfd79a10f7fa | aba0fd730ddd52e2a3700ff6381a17c40e2bdf29 | 3.53.2 |
| noop_gravity_witness | before | b647bcba9fa5f8eb583b162f87f7dfd79a10f7fa | aba0fd730ddd52e2a3700ff6381a17c40e2bdf29 | 3.53.2 |
| openwhispr_speaker_words | before | b647bcba9fa5f8eb583b162f87f7dfd79a10f7fa | aba0fd730ddd52e2a3700ff6381a17c40e2bdf29 | 3.53.2 |
| phoenix_token_rollup | before | b647bcba9fa5f8eb583b162f87f7dfd79a10f7fa | aba0fd730ddd52e2a3700ff6381a17c40e2bdf29 | 3.53.2 |
| pocketbase_author_counts | before | b647bcba9fa5f8eb583b162f87f7dfd79a10f7fa | aba0fd730ddd52e2a3700ff6381a17c40e2bdf29 | 3.53.2 |
| postio_mailbox_footprint | before | b647bcba9fa5f8eb583b162f87f7dfd79a10f7fa | aba0fd730ddd52e2a3700ff6381a17c40e2bdf29 | 3.53.2 |
| zcash_transparent_balance | before | b647bcba9fa5f8eb583b162f87f7dfd79a10f7fa | aba0fd730ddd52e2a3700ff6381a17c40e2bdf29 | 3.53.2 |
| zenod_usage_totals | before | b647bcba9fa5f8eb583b162f87f7dfd79a10f7fa | aba0fd730ddd52e2a3700ff6381a17c40e2bdf29 | 3.53.2 |
| nicti_facet_counts | after | 84e4c07c2ebfc9db8b6bafb57378627414df5524 | aba0fd730ddd52e2a3700ff6381a17c40e2bdf29 | 3.53.2 |
| noop_gravity_witness | after | 84e4c07c2ebfc9db8b6bafb57378627414df5524 | aba0fd730ddd52e2a3700ff6381a17c40e2bdf29 | 3.53.2 |
| openwhispr_speaker_words | after | 84e4c07c2ebfc9db8b6bafb57378627414df5524 | aba0fd730ddd52e2a3700ff6381a17c40e2bdf29 | 3.53.2 |
| phoenix_token_rollup | after | 84e4c07c2ebfc9db8b6bafb57378627414df5524 | aba0fd730ddd52e2a3700ff6381a17c40e2bdf29 | 3.53.2 |
| pocketbase_author_counts | after | 84e4c07c2ebfc9db8b6bafb57378627414df5524 | aba0fd730ddd52e2a3700ff6381a17c40e2bdf29 | 3.53.2 |
| postio_mailbox_footprint | after | 84e4c07c2ebfc9db8b6bafb57378627414df5524 | aba0fd730ddd52e2a3700ff6381a17c40e2bdf29 | 3.53.2 |
| zcash_transparent_balance | after | 84e4c07c2ebfc9db8b6bafb57378627414df5524 | aba0fd730ddd52e2a3700ff6381a17c40e2bdf29 | 3.53.2 |
| zenod_usage_totals | after | 84e4c07c2ebfc9db8b6bafb57378627414df5524 | aba0fd730ddd52e2a3700ff6381a17c40e2bdf29 | 3.53.2 |
| fluxflow_grouped_flow_rollup | before | b647bcba9fa5f8eb583b162f87f7dfd79a10f7fa | b647bcba9fa5f8eb583b162f87f7dfd79a10f7fa | 3.53.2 |
| fluxflow_grouped_flow_rollup | after | 84e4c07c2ebfc9db8b6bafb57378627414df5524 | aba0fd730ddd52e2a3700ff6381a17c40e2bdf29 | 3.53.2 |
| kener_quarter_hour_rollup | before | b647bcba9fa5f8eb583b162f87f7dfd79a10f7fa | b647bcba9fa5f8eb583b162f87f7dfd79a10f7fa | 3.53.2 |
| kener_quarter_hour_rollup | after | 84e4c07c2ebfc9db8b6bafb57378627414df5524 | aba0fd730ddd52e2a3700ff6381a17c40e2bdf29 | 3.53.2 |
