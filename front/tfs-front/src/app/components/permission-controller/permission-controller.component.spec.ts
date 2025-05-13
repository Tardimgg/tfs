import { ComponentFixture, TestBed } from '@angular/core/testing';

import { PermissionControllerComponent } from './permission-controller.component';

describe('PermissionControllerComponent', () => {
  let component: PermissionControllerComponent;
  let fixture: ComponentFixture<PermissionControllerComponent>;

  beforeEach(async () => {
    await TestBed.configureTestingModule({
      imports: [PermissionControllerComponent]
    })
    .compileComponents();

    fixture = TestBed.createComponent(PermissionControllerComponent);
    component = fixture.componentInstance;
    fixture.detectChanges();
  });

  it('should create', () => {
    expect(component).toBeTruthy();
  });
});
