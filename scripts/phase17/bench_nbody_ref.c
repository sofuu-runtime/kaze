/* Phase 17 benchmark reference: the same scalar N-body loop in C99. */
#include <math.h>
#include <stdio.h>

static double integrate(long steps) {
    double x = 0.1, y = 0.2, z = 0.3;
    double vx = 0.01, vy = 0.02, vz = 0.03;
    double dt = 0.000001;
    for (long i = 0; i < steps; i++) {
        double r2 = x * x + y * y + z * z;
        double inv = (r2 != 0.0) ? (1.0 / sqrt(r2)) : 0.0;
        double f = inv * inv * inv;
        vx = vx - x * f * dt;
        vy = vy - y * f * dt;
        vz = vz - z * f * dt;
        x = x + vx * dt;
        y = y + vy * dt;
        z = z + vz * dt;
    }
    return x + y + z;
}

int main(void) {
    printf("%.17g\n", integrate(1000000));
    return 0;
}
